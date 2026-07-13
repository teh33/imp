# Compaction architecture audit

Date: 2026-07-13
Scope: manual compaction, automatic request compaction, durable session projection, overflow recovery, configuration, and tests

## Conclusion

imp's compaction feature needs a redesign, not another prompt adjustment.

The durable JSONL session and branch-local replacement model are useful foundations. The current compaction pipeline, however, treats operational state transfer as lossy transcript summarization. It removes evidence before summarization, does not validate summaries against authoritative runtime state, has divergent manual and automatic paths, and can resolve the wrong durable boundary after repeated manual compaction.

The current readiness rollup of 69% is not supported by the implementation or tests. An evidence-based assessment is approximately 48%: useful single-pass mechanics, but incomplete correctness and durability for long-running work.

## Current architecture

### Manual `/compact`

1. The TUI reads `SessionManager::get_active_messages()`.
2. `prepare_messages_for_compaction` partitions active messages by assistant-action groups.
3. Every compacted tool-result body is replaced by a small placeholder.
4. The reduced prefix is serialized into a prose prompt.
5. A model may generate a summary; timeout, oversize input, or an empty response selects a deterministic fallback.
6. `execute_manual_compaction_with_prompt_options` maps an active-message index back to a raw session message ID.
7. A `SessionEntry::Compaction` stores the summary, first kept ID, and approximate token counts.
8. `get_active_messages()` replaces the old raw prefix with the latest synthetic summary.

Primary paths:

- `crates/imp-tui/src/app/compaction.rs`
- `crates/imp-core/src/compaction.rs`
- `crates/imp-core/src/session/mod.rs`

### Automatic compaction

Automatic compaction runs inside the agent request loop after ordinary observation masking. It:

- operates on request-projected messages rather than the durable session;
- uses a deterministic digest rather than the configured summarizer;
- partitions on user-message boundaries;
- rewrites only the current provider request;
- does not append a durable compaction entry;
- is reconstructed from canonical history on later turns.

Primary path: `crates/imp-core/src/agent/run_loop.rs:267-359`.

### Overflow recovery

If the resulting request is still over budget, the runtime masks observations in `self.messages` once and retries context assembly. If it remains too large, the run fails explicitly with a context-full diagnostic.

Primary path: `crates/imp-core/src/agent/run_loop.rs:361-430`.

## Findings

### Critical: repeated manual compaction can store the wrong boundary

`prepare_messages_for_compaction` returns `preserved_tail_start` as an index into `get_active_messages()`. After a prior compaction, active messages begin with a synthetic summary that has no corresponding raw `SessionEntry::Message`.

`execute_manual_compaction_with_prompt_options` then counts raw branch messages from the beginning and uses the active index to choose `first_kept_id`:

- active projection: `crates/imp-core/src/session/mod.rs:837-881`;
- index-to-ID mapping: `crates/imp-core/src/compaction.rs:760-780`.

The two coordinate systems differ after the first compaction. A second compaction can therefore choose an older or otherwise unintended raw message as its kept boundary. This can reintroduce previously compacted history or produce a tail different from the one used to calculate the summary and token counts.

No test executes repeated manual compaction.

Required correction: compaction planning must carry stable session entry IDs throughout. It must never derive a durable boundary by applying an active-projection index to the raw branch.

### Critical: evidence is destroyed before summarization

`shrink_messages_for_summary` replaces every older tool result with only:

- tool name;
- truncated arguments;
- output byte count.

See `crates/imp-core/src/compaction.rs:114-155`.

The later prompt asks the model to preserve commands, results, errors, edits, verification, artifact paths, and current state. Those facts may already be absent. A better prompt cannot recover discarded evidence.

This is especially unsafe for:

- failed verification and exact error messages;
- edit results and changed-file metadata;
- Git status and branch evidence;
- tool-produced artifact paths;
- policy or approval decisions;
- unresolved blockers discovered in tool output.

Required correction: extract typed, durable facts before reducing transcript payloads. Preserve structured tool-result details and bounded evidence selectively rather than dropping every result uniformly.

### Critical: generated summaries are activated without reconciliation

The runtime does not parse or validate a generated summary against:

- task objective and constraints;
- workflow state;
- obligations and blockers;
- repository or branch state;
- edited paths;
- verification evidence;
- session boundaries.

The model can omit a hard constraint or falsely state that work is complete, and the result immediately becomes the active historical projection.

Required correction: summaries must be generated from a typed continuation state and checked against that state before activation. Invalid summaries should be repaired, rejected, or replaced by a deterministic rendering of authoritative state.

### High: manual and automatic compaction have incompatible semantics

Manual and automatic compaction differ in persistence, grouping, summarization, inputs, and lifecycle. The same conversation can receive materially different continuation context depending on whether a user invokes `/compact` or threshold pressure triggers automatic compaction.

The completed `codex-style-compaction-workflow` explicitly designed an event-mediated durable automatic transition, but the current request-local implementation still does not persist `SessionEntry::Compaction`. Its results artifact already records this concern at `.imp/workflows/codex-style-compaction-workflow/results.md:106-143`.

Required correction: both entry points should invoke one canonical planner and state transition. UI and threshold policy may differ; compaction semantics should not.

### High: fallback output is a digest, not operational state

`build_fallback_summary` keeps:

- up to ten early user prompts;
- up to 18,000 characters selected newest-first;
- up to forty recent tool calls;
- generic tool success/error lines.

See `crates/imp-core/src/compaction.rs:374-489`.

It has no enforced representation for acceptance criteria, current decisions, edited paths, verification state, obligations, blockers, or workflow status. Repeated fallback compaction compounds omission.

Required correction: deterministic fallback should render the same typed continuation state used by model-assisted compaction.

### High: the overflow retry contract is ineffective

`execute_compaction_with_retry_and_prompt_options` increases `keep_recent_groups` when execution returns `None`. This preserves more raw history and makes compaction less aggressive.

In the TUI, an oversized summarization prompt returns `Ok(None)`, but the executor interprets that as permission to use the deterministic fallback and returns `Some` immediately. The advertised retry therefore does not reduce oversized summarizer input in the common failure path.

See:

- `crates/imp-core/src/compaction.rs:832-858`;
- `crates/imp-tui/src/app/compaction.rs:145-158`.

Required correction: distinguish `NotNeeded`, `SummaryUnavailable`, `InputTooLarge`, and `Compacted`. Retry policy should reduce summary input or choose deterministic state rendering explicitly.

### High: summarizer configuration is misleading

`SummarizerConfig.model` is not used outside configuration definitions. Manual compaction always uses the active conversation model.

The default target is 40,000 summary tokens, while TUI generation defaults to 2,048 output tokens and is capped at 4,096. The reserve setting limits prompt input, not generated-summary size.

See:

- `crates/imp-core/src/config.rs:578-613`;
- `crates/imp-tui/src/app/compaction.rs:80-93`;
- `crates/imp-tui/src/app/compaction.rs:172-180`.

Required correction: either implement summarizer model routing and coherent input/output budgets or remove the inert and impossible configuration.

### Medium: token accounting is not one contract

Manual compaction estimates serialized message JSON with a generic estimator. Automatic compaction uses model-aware context accounting. Post-compaction manual counts exclude the full request envelope, including system prompt, tools, planned output, and provider-specific limits.

The TUI's “saved tokens” result is therefore not comparable to provider preflight usage.

Required correction: use one request-aware budget model and label provider usage, local estimates, and unknown post-compaction usage distinctly.

### Medium: strategy selection is speculative API surface

`select_compaction_strategy` reports provider-native support for known provider IDs when enabled, but no provider-native path is implemented and the TUI always passes `allow_provider_native: false`.

Required correction: remove the premature seam or make capability selection depend on an implemented provider contract rather than provider-name matching.

## Existing strengths

Preserve these properties during the redesign:

- raw durable messages are not deleted by manual compaction;
- compaction markers participate in branch/fork history;
- active history replacement is explicit;
- provider requests sanitize tool-call/result invariants;
- automatic compaction does not silently mutate the persisted transcript;
- unresolved overflow eventually fails explicitly;
- focused tests cover many single-pass mechanics.

## Proposed architecture

### Typed continuation state

Introduce a versioned `ContinuationState` assembled from runtime-owned sources before transcript reduction:

```text
ContinuationState
├── objective and acceptance criteria
├── active user constraints and corrections
├── decisions with rationale and provenance
├── repository, branch, and session state
├── edited files and effect records
├── verification commands and outcomes
├── pending obligations
├── blockers, risks, and open questions
├── workflow/task identifiers and status
└── artifact and evidence references
```

The state should use stable IDs and provenance. Summary prose is a presentation derived from this state, not the state itself.

### Canonical compaction plan

Manual and automatic entry points should produce one `CompactionPlan` containing:

- compacted entry IDs;
- preserved entry IDs;
- resolved input/output budget;
- extracted continuation state;
- selected bounded evidence excerpts;
- strategy and model identity;
- expected post-compaction request size.

The session layer should apply this plan atomically and verify that every referenced entry belongs to the active branch.

### Evidence-aware reduction

Classify observations by continuation value:

- preserve concise errors and failed verification;
- preserve structured edit and diff metadata;
- preserve artifact paths and identifiers;
- preserve current Git and workflow state;
- reference large file or command dumps by artifact;
- discard redundant content only after durable facts are extracted.

### Validated summary generation

Generate schema-constrained continuation output. Validate required state items before activation. On failure:

1. request a bounded repair when budget permits;
2. otherwise render `ContinuationState` deterministically;
3. never silently activate a summary known to omit or contradict authoritative state.

### Versioned persistence

A new compaction record should include:

- schema version;
- compacted and preserved entry ID sets or ranges;
- typed continuation state or a durable reference to it;
- rendered summary;
- strategy and model;
- request-aware token estimates;
- source-state fingerprint;
- validation result.

Read old compaction entries for compatibility. Write only the new form after migration.

## Implementation order

1. Add a repeated-compaction regression that demonstrates the current boundary defect.
2. Replace active-index/raw-index mapping with stable entry IDs.
3. Define `ContinuationState` and deterministic rendering.
4. Extract task/workflow/effect/verification state before transcript reduction.
5. Introduce one canonical `CompactionPlan` for manual and automatic paths.
6. Persist automatic compaction through the session owner.
7. Add schema-constrained generation and reconciliation.
8. Make budgets request-aware and configuration coherent.
9. Add quality evaluations and only then tune prompts or thresholds.
10. Consider provider-native compaction after local state semantics are correct.

## Required verification

Add tests for:

1. repeated manual compaction over a long session;
2. save, reload, and continue after every compaction cycle;
3. branch/fork semantics after multiple compactions;
4. exact stable-ID boundary selection;
5. hard user constraints and corrections surviving every cycle;
6. failed tests and pending obligations surviving compaction;
7. rejection of summaries that falsely claim completion;
8. structured metadata retained when large output bodies are removed;
9. equivalent state semantics for manual and automatic triggers;
10. provider overflow below a local estimate;
11. cumulative omission over several compaction cycles;
12. summary targets fitting actual model output caps.

Quality evaluation should score factual recall, contradiction, obligation preservation, resumption success, post-compaction completion correctness, tokens saved, and latency. Token reduction alone is not a sufficient success metric.

## Audit evidence

Focused verification:

```text
cargo nextest run -p imp-core compaction --no-fail-fast
23 tests passed; 0 failed; 1019 skipped by filter
```

A source search found no repeated-compaction test.

The existing readiness ledger reports 69% for compaction. A dry-run reassessment based on this audit produced approximately 48%, with gaps in state preservation, conflict detection, authoritative reload, and repeated-compaction proof. Applying the reassessment caused the current `ready` version to normalize default `value` fields throughout the legacy ledger, producing a 4,743-line unrelated diff. That mutation was reverted rather than committing broad serialization churn.

No product source was modified during this audit.
