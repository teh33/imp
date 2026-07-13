# Compaction Rework Technical Design

## Current behavior and constraints

### Session projection

`SessionManager::get_active_messages()` reads the latest legacy compaction marker and returns one synthetic summary message followed by raw messages beginning at `first_kept_id`. Raw branch entries remain on disk.

Manual compaction prepares `get_active_messages()`, receives a `preserved_tail_start` message index, then counts raw branch messages to derive `first_kept_id`. After an earlier compaction, the active projection contains a synthetic summary with no raw message entry, so repeated compaction mixes coordinate systems.

### Runtime ownership

`Agent` owns request assembly, threshold decisions, task state, obligations, and provider calls. It does not own `SessionManager`. `ImpSession` owns session persistence and consumes `AgentEvent`. This boundary should remain: the agent proposes a transition; the session owner validates branch identity and persists it.

### Current summarization

Manual TUI compaction directly builds a second agent using the active conversation model. `SummarizerConfig.model` is unused. Older tool-result bodies are removed before summarization, generated prose is not parsed or validated, and empty/oversized/timeout responses select a deterministic fallback.

Automatic compaction is a separate request-local deterministic rewrite in `Agent::run`. It is not persisted and has different grouping and failure semantics.

### Model routing

`gpt-5.6-luna` is a built-in OpenAI model with a 1.05M context window, 128k output limit, and lower listed pricing than Sol/Terra. Existing runtime connection resolution can select an API-key OpenAI route or an explicitly supported OpenAI Codex OAuth route. The compactor must use that resolver rather than forcing a provider.

## Proposed design

### Module boundaries

Split the current compaction godfile into purposeful modules under `crates/imp-core/src/compaction/`:

- `mod.rs`: public types and orchestration exports;
- `plan.rs`: stable-ID source partition and budget plan;
- `state.rs`: typed continuation state and deterministic extraction;
- `evidence.rs`: selective observation reduction;
- `prompt.rs`: versioned Luna request schema and prompt;
- `validate.rs`: response parsing and authoritative-state reconciliation;
- `legacy.rs`: legacy compaction compatibility and migration projection;
- `tests.rs`: focused pure tests, with persistence tests remaining near session tests.

No authored source file should exceed repository size limits.

### Stable active entries

Add an internal session projection that preserves identity:

```rust
pub struct ActiveSessionMessage {
    pub source: ActiveMessageSource,
    pub message: Message,
}

pub enum ActiveMessageSource {
    Compaction { entry_id: String },
    Message { entry_id: String },
}
```

`get_active_messages()` remains compatible by stripping identity. Compaction planning consumes identified active messages. A kept boundary must be an actual `Message` entry ID on the active branch. The synthetic prior compaction representation may be compacted into the next state, but can never be mistaken for a raw message index.

### Typed continuation state

Define versioned serializable records with explicit provenance:

```rust
pub struct ContinuationStateV1 {
    pub objective: Vec<StateFact>,
    pub constraints: Vec<StateFact>,
    pub decisions: Vec<StateFact>,
    pub repository_state: Vec<StateFact>,
    pub effects: Vec<StateFact>,
    pub verification: Vec<VerificationFact>,
    pub obligations: Vec<StateFact>,
    pub blockers: Vec<StateFact>,
    pub workflow_state: Vec<StateFact>,
    pub artifacts: Vec<StateFact>,
}

pub struct StateFact {
    pub id: String,
    pub text: String,
    pub provenance: FactProvenance,
    pub required: bool,
}
```

The initial extractor combines:

- identified session messages and structured tool-result details;
- `SessionTaskState` snapshot;
- obligation ledger snapshot;
- workflow controller/runtime snapshot when available;
- current repository facts already recorded by tools/session state.

Do not run new repository commands from pure compaction planning. Exact live state refresh belongs to normal runtime/tool execution.

### Compaction plan

```rust
pub struct CompactionPlan {
    pub trigger: CompactionTrigger,
    pub source_entry_ids: Vec<String>,
    pub preserved_entry_ids: Vec<String>,
    pub first_kept_id: Option<String>,
    pub continuation: ContinuationStateV1,
    pub evidence: Vec<EvidenceExcerpt>,
    pub budget: CompactionBudget,
    pub source_fingerprint: String,
}
```

Planning is deterministic and side-effect free. The session owner checks that source and preserved IDs still describe the active branch before persistence, preventing stale async compaction from overwriting steered or newly appended state.

### Evidence selection

Tool results are reduced by semantic class:

- retain structured `files`, diff summaries, exit codes, error indicators, artifact paths, and policy/provenance labels;
- retain bounded exact text for failed checks and concise errors;
- retain tool name and key arguments;
- reference large outputs by artifact when present;
- omit bulk successful file/command output after facts are extracted.

Unknown tool output is bounded but not silently interpreted as success.

### Rolling disk checkpoints

Store staged checkpoints in session-owned storage outside the JSONL transcript. Each checkpoint contains its schema version, covered entry IDs, prior checkpoint ID, source and state fingerprints, typed continuation state, rendered summary, evidence references, model route, thinking level, token usage, and validation result.

Checkpoint scheduling uses `context.summarizer.checkpoint_interval_tokens`, defaulting to 128,000 uncovered input tokens. Runtime events only mark source state dirty. Generation begins after the turn becomes idle and only when the interval has been reached. A single-flight worker snapshots source state, writes to a temporary sibling, validates, fsyncs, and atomically renames the checkpoint. The prior valid checkpoint remains available until replacement succeeds.

Incremental generation sends Luna the last validated checkpoint plus uncovered entries and authoritative state changes. A final compaction trigger uses the latest checkpoint plus the remaining raw tail; it calls Luna again only when that tail cannot safely remain verbatim.

### Luna request and response

Set the shipped `SummarizerConfig.model` default to `gpt-5.6-luna`, `thinking` to `xhigh`, and `checkpoint_interval_tokens` to 128,000. Keep `default` as an explicit compatibility value meaning active conversation model. `system_prompt` fully replaces the built-in compaction system prompt.

Resolve a `CompactorConnection` through the same model-first registry/auth policy as normal sessions. Resolution must return the actual model, provider route, and credentials. Failure is typed and visible.

Send Luna:

- the typed continuation state;
- selected evidence excerpts;
- required fact IDs;
- source/preserved boundary metadata;
- a strict JSON schema for `CompactionDocumentV1`;
- a realistic output budget derived from configuration and model limits.

The response contains a concise rendered continuation plus an acknowledgement list of preserved fact IDs. The runtime does not request or retain hidden thinking.

### Validation

Validation occurs before any session mutation:

1. Parse the exact versioned JSON response.
2. Reject unknown schema versions or malformed output.
3. Require every `required` fact ID to be acknowledged.
4. Reject unsupported completion claims when verification or obligations remain unresolved.
5. Confirm boundary IDs and source fingerprint still match the active branch.
6. Build the candidate active history and sanitize tool protocol order.
7. Estimate the complete provider request against the target budget.
8. Reject candidates that do not reduce context or do not fit.

The rendered summary is presentation. The persisted typed continuation state remains authoritative.

### Persistence

Add a backward-compatible compaction entry variant rather than changing the legacy variant's required shape:

```rust
SessionEntry::CompactionV2 {
    id,
    parent_id,
    record: CompactionRecordV2,
}
```

`CompactionRecordV2` embeds the plan boundary, continuation state, rendered summary, model/provider identity, trigger, source fingerprint, validation metadata, and token estimates.

Projection uses the latest V1 or V2 marker. V2 uses stable preserved IDs. Legacy V1 retains current behavior for old files. New code writes V2 only.

### Canonical coordinator

Introduce an async compaction coordinator owned by `ImpSession`, because it has model registry, auth, and mutable session persistence. The coordinator:

1. snapshots the identified active branch and runtime state;
2. builds a deterministic plan;
3. resolves and calls Luna;
4. validates output;
5. rechecks the active branch fingerprint;
6. appends V2 atomically;
7. reloads active messages into the agent.

Manual TUI `/compact` calls this session API rather than constructing a separate ad hoc agent.

Automatic threshold handling moves from request-local rewriting to a structured request from `Agent` to its session owner. The minimal transport is a command/event handshake with a bounded response channel. The agent pauses before the provider request while `ImpSession` coordinates compaction, then receives either refreshed messages or a typed failure.

If that handshake is too invasive for one patch, first land the coordinator and manual path, then route automatic compaction through it before removing the request-local implementation. Do not claim semantic unification until the latter lands.

### Required legacy deletion

Once the V2 coordinator handles both triggers, delete `build_fallback_summary`, `compact_messages_for_auto_compaction`, optional-summary fallback execution, retry behavior that increases the preserved tail, `select_compaction_strategy`, and TUI-owned ad hoc compaction construction. Retain only narrowly named V1 record projection code. Tests asserting deterministic fallback behavior must be deleted or replaced with strict unchanged-context failure tests.

## Failure handling

### Manual

Any resolution, auth, provider, timeout, parse, validation, stale-plan, persistence, or budget failure returns `CompactionError`. No marker is appended. The TUI retains current messages and displays the typed reason.

### Automatic

The agent records whether the original sanitized request fits the effective hard input limit before attempting compaction.

- On success, it sends refreshed durable active history.
- On failure while the original fits, it emits `CompactionFailed`, sends the original request once, and suppresses another automatic attempt for that turn.
- On failure while the original does not fit, it emits the combined compaction/context error and terminates.

No alternate model or deterministic summary is activated.

### Cancellation and steering

Compaction is cancellable. A source fingerprint prevents completion against stale history. Steering or appended messages invalidate the pending plan; the coordinator returns `StalePlan` without mutation.

## Compatibility, migration, rollout, rollback

- Read V1 and V2; write V2.
- Keep `get_active_messages()` public behavior stable.
- Preserve `context.summarizer.model = "default"` semantics explicitly.
- Change the default only for configurations that omit the field; existing explicit values remain unchanged.
- Land behind the existing automatic-compaction mode. Manual compaction can adopt V2 first.
- Rollback remains possible because raw messages are retained and old binaries ignore unknown entries only if current session decoding supports it. Before shipping V2, add a compatibility fixture and decide whether unknown enum variants require a session format version bump. If old binaries cannot read V2, document minimum-version compatibility and provide a projection/export path.

## Security and policy considerations

- Luna receives only the same session content allowed for the active provider route plus bounded runtime state; no unrelated project/session memory is added.
- Tool outputs and external content remain data, not instructions. The compaction system prompt states this boundary.
- Credentials stay in existing auth stores and are never persisted in compaction records.
- Provenance and policy labels survive evidence extraction.
- Generated output cannot authorize tools, writes, policy changes, or completion.

## Alternatives and tradeoffs

### Luna-only prose summary

Rejected. It improves likely prose quality but leaves authority, repeated-boundary correctness, and contradiction risks unresolved.

### Deterministic-only compaction

Rejected as the primary product direction. It is reliable but cannot compress nuanced long-session context as effectively. Deterministic typed state remains the authority and validation fallback representation, but strict selected-model failure means it is not silently activated as a substitute summary.

### Direct Agent ownership of session storage

Rejected. It couples provider-loop logic to persistence and weakens adapter consistency. `ImpSession` already owns the correct state-transition boundary.

### Provider-native compaction now

Deferred. Local semantics and durable authority must be correct before remote window state is introduced.

## Verification strategy

### Unit

- stable-ID partition across first and repeated compaction;
- continuation extraction from task, verification, tool details, and artifacts;
- evidence bounding;
- strict schema parsing and required-fact validation;
- budget validation and no-reduction rejection;
- typed failure policy.

### Persistence

- V1 fixture remains readable;
- V2 round trip and restart projection;
- repeated V2 compaction;
- fork from before and after V2 marker;
- stale source fingerprint rejection;
- no entry on any failure.

### Runtime

- manual Luna success and every strict failure class;
- automatic failure continues only when original request fits;
- automatic failure terminates when it does not;
- no second automatic attempt in the same turn;
- successful automatic compaction persists and survives restart.

### Model routing

- default resolves Luna;
- explicit `default` resolves active model;
- API-key and supported OAuth routes;
- unsupported route fails without model substitution;
- configured unknown model fails before mutation.

### Quality evaluation

Use deterministic long-session fixtures and optional live Luna runs. Score required-fact recall, contradiction rate, obligation preservation, restart/resumption success, completion correctness, context reduction, latency, and cost. Do not gate correctness on live nondeterministic output alone.
