# Runtime state authority

## Decision

`RuntimeEvent` and `RuntimeStateAccumulator` are the domain execution-state boundary. Clients consume reducer snapshots and bounded deltas. The TUI no longer interprets `AgentEvent` twice to maintain transcript, tools, phase, verification, usage, policy warnings, evidence, worktree status, errors, or final status.

The production flow is:

```text
AgentEvent
  -> RuntimeEvent (schema_version, run_id, sequence)
  -> RuntimeStateAccumulator
  -> RuntimeStateDelta + RuntimeStateSnapshot
  -> client projection
  -> rendering / protocol output
```

The previous TUI path first reduced a lossy runtime event and then called a legacy handler that independently mutated the same domain fields. The remaining adapter is `handle_runtime_ui_effects`. It reacts to authoritative transitions for UI behavior only: focus, scroll following, thought-duration presentation, persistence, completion notification, queued prompts, and local turn tracking.

## Event schema and compatibility

The wire schema remains version 1. Existing names such as `agent_started`, `agent_ended`, `message_delta`, `tool_started`, `tool_output`, `tool_completed`, warning, error, workflow, verification, evidence, worktree, timing, and recovery remain readable. Changes are additive:

- `assistant_delta` distinguishes visible text and thinking.
- `message_observed`, `message_finalized`, and `session_hydrated` preserve message boundaries and resumed transcript state.
- `tool_declared` is distinct from tool execution start.
- structured context usage, turn completion, verification completion, policy warning class, worktree notices, and recovery summaries retain data previously flattened into strings.
- final usage includes integer micro-cost fields as well as the compatible formatted total.

Unknown event kinds deserialize to `RuntimeEventKind::Unknown`. They advance the accepted sequence and revision, record the unknown name, and do not alter execution state. A newer schema version returns `UnsupportedSchema` before mutation. Sensitive argument keys including token, secret, password, authorization, cookie, API key, and generic secret value fields are recursively redacted during `AgentEvent` conversion.

## Sequence and replay semantics

Every applied event requires a non-empty run ID and sequence number.

- The first valid event establishes the run if the accumulator was unbound.
- Strictly increasing events apply normally.
- An event at the last applied sequence is `Duplicate` and is ignored.
- A sequence below the last sequence is `Stale` and is ignored.
- A gap is accepted as `Gap { expected, received, delta }`, applies once, and records a bounded warning.
- A different run is `RunMismatch` and cannot mutate the active run.
- An empty run ID is `EmptyRunId`.
- A newer schema is `UnsupportedSchema`.

`RuntimeApplyOutcome` makes each choice explicit. Accepted events increment a snapshot revision. Replaying the same ordered events into a fresh accumulator produces the same snapshot.

## State ownership

`RuntimeStateSnapshot` owns:

- run, model, phase, turn status, and final status
- ordered transcript messages and assistant blocks
- visible text, thinking, and tool references
- active/completed tools and bounded output summaries
- pending/resolved approvals and policy decisions
- verification gates and closeout effects
- evidence, child workflows, workflow controller, worktree, and recovery summaries
- usage, context usage, integer micro-costs, bounded warnings, errors, and status facts

The TUI keeps presentation and interaction state:

- editor, cursor, focus, overlays, selection, clipboard, command palette
- scroll anchors, layout rectangles, animation ticks, and render caches
- selected/pinned tool presentation and sidebar auto-follow
- queued local gestures/prompts, notification effects, and thought-duration timing
- display projections used by Ratatui

Runtime types have no Ratatui dependency.

## Transcript and tool projection

A transcript message has a typed role, stable reducer ID, streaming/final state, optional timestamp, and ordered blocks. Assistant blocks are visible text, thinking, or a tool-call reference. Adjacent deltas of the same kind coalesce, but text/tool/text/tool/text ordering is retained. Provider errors can replace a pending assistant message before visible assistant output.

The TUI updates only the message or tool named in `RuntimeStateDelta`; it does not clone or rebuild the full transcript per token. Sanitized durable session messages are projected directly into runtime-domain transcript/tool records, then rendered through the same TUI projection as live execution, preserving resumed-session behavior without a display-to-domain round trip.

Tool output is a client-facing tail, not a durable log. `MAX_RUNTIME_TOOL_OUTPUT_CHARS` bounds it to 16,384 characters and marks truncation. Full output remains in session and tool artifacts.

## Deltas and performance

`RuntimeStateDelta` identifies changed state families and the changed message/tool ID. The TUI uses it for incremental projection and cache invalidation. Runtime signal batching remains bounded at 256 signals; dirty rendering, skipped animation ticks, sidebar caches, and queue bounds are unchanged. Applying a token delta mutates one transcript block and one display projection.

## ACP and other clients

The legacy `AgentEvent` contract and ACP conversion remain intact. Runtime-domain transcript, tool, usage, verification, and final-status models are client-neutral and reusable by ACP, JSON/RPC, and future GUI clients. This change does not require ACP or Ratatui types in core.

## Process-runtime extension point

This change deliberately defines no process lifecycle payload. The process-runtime branch should add typed runtime event variants only after its typed `ProcessEvent` is available, then implement one explicit mapping at the client/runtime boundary:

```text
ProcessEvent -> versioned RuntimeEventKind::<typed process variant> -> reducer state/delta
```

It must use the parent run ID and sequence allocator, preserve redaction and bounded-output rules, and avoid importing process implementation types into the TUI. Unknown process variants remain safe through the existing unknown-kind compatibility path.
