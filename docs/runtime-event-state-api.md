# Runtime Event and State API

## Goal

Define a shared runtime event/state contract for CLI, TUI, RPC, tests/replay,
and future GUI consumers. The contract lives in `imp-core` and keeps semantic
runtime facts out of frontend-specific state machines.

The implemented core surface has three pieces:

1. `RuntimeEvent`: a versioned, append-only event payload for streaming/logging.
2. `RuntimeStateSnapshot`: a versioned, frontend-neutral current-state snapshot.
3. `RuntimeStateAccumulator`: a deterministic reducer from ordered runtime
   events into a snapshot.

Existing `AgentEvent` consumers remain supported. Runtime events are an adapter
layer for shared frontend/RPC/GUI state, not a replacement for the agent loop.

## Module and schema versioning

The implemented types are exported from:

```rust
imp_core::runtime
```

The schema version is explicit:

```rust
pub const RUNTIME_SCHEMA_VERSION: u32 = 1;
```

Both `RuntimeEvent` and `RuntimeStateSnapshot` include `schema_version`. Any
future breaking schema change must deliberately bump this value and update
compatibility tests.

## RuntimeEvent

Implemented shape:

```rust
pub struct RuntimeEvent {
    pub schema_version: u32,
    pub run_id: String,
    pub sequence: u64,
    pub timestamp_ms: Option<u64>,
    pub kind: RuntimeEventKind,
}
```

`RuntimeEventKind` is serde-tagged with `type` and snake_case variant names.
Current event coverage:

- `agent_started`
- `agent_ended`
- `turn_started`
- `turn_assessed`
- `turn_ended`
- `message_started`
- `message_delta`
- `message_ended`
- `tool_started`
- `tool_output`
- `tool_completed`
- `approval_pending`
- `approval_resolved`
- `policy_decision`
- `verification_updated`
- `evidence_updated`
- `worktree_updated`
- `workflow_updated`
- `warning`
- `error`
- `timing`
- `recovery_checkpoint`
- `unknown`

`RuntimeEventKind::Unknown` exists so future/foreign event streams can be tracked
without corrupting accumulated state.

## AgentEvent compatibility

`AgentEvent` remains the internal streaming compatibility surface. The adapter is
implemented as:

```rust
impl AgentEvent {
    pub fn to_runtime_event(&self, run_id: impl Into<String>, sequence: u64) -> RuntimeEvent;
}
```

The adapter maps lifecycle, turns, messages, tools, warnings/errors, timing,
recovery checkpoints, verification, worktree metadata/closeout, evidence refs,
and policy decisions into typed runtime payloads. Existing trace/RPC/TUI
`AgentEvent` paths continue to work while consumers migrate to runtime events and
snapshots.

## RuntimeStateSnapshot

Implemented shape:

```rust
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    pub workflow: RuntimeWorkflowSummary,
    pub autonomy_mode: Option<AutonomyMode>,
    pub workspace: RuntimeWorkspaceState,
    pub phase: RuntimePhase,
    pub active_tools: Vec<RuntimeToolCall>,
    pub completed_tools: Vec<RuntimeToolCall>,
    pub pending_approvals: Vec<RuntimeApprovalRef>,
    pub policy_decisions: Vec<RuntimePolicyDecision>,
    pub verification_gates: Vec<VerificationGate>,
    pub evidence_refs: Vec<RuntimeArtifactRef>,
    pub final_status: Option<RuntimeFinalStatus>,
    pub workflow_refs: Vec<RuntimeWorkflowRef>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub status_items: BTreeMap<String, String>,
}
```

The snapshot answers reusable frontend questions:

- which run/model is active
- current phase/final status
- active and completed tools
- pending approvals
- policy decisions and warnings/errors
- verification gates
- evidence artifacts
- worktree path/branch/diff/closeout state
- workflow refs
- compact status items useful for CLI/TUI/GUI rendering

## RuntimeStateAccumulator

Implemented reducer:

```rust
pub struct RuntimeStateAccumulator { /* private snapshot */ }

impl RuntimeStateAccumulator {
    pub fn new(run_id: impl Into<String>) -> Self;
    pub fn from_snapshot(snapshot: RuntimeStateSnapshot) -> Self;
    pub fn apply(&mut self, event: &RuntimeEvent);
    pub fn snapshot(&self) -> RuntimeStateSnapshot;
}
```

The accumulator is deterministic and side-effect free. It tracks lifecycle,
model/final status, active/completed tools, tool output deltas, approvals, policy
decisions, verification gates, evidence refs, worktree scope/status/diff/closeout,
workflow refs, warnings/errors, timing/recovery status, and unknown future events.

Unknown runtime events are recorded in `status_items["last-unknown-event"]` and do
not change phase.

## TUI state mapping

Current TUI state in `imp-tui/src/app.rs` and `turn_tracker.rs` maps to the core
snapshot like this:

| Current TUI state | Runtime owner | Snapshot field |
| --- | --- | --- |
| model/run/phase status labels | core facts + TUI formatting | `workflow`, `phase`, `status_items` |
| `TurnTracker` tool counts | core | `active_tools`, `completed_tools`, `status_items` |
| tool output previews | core semantic preview + TUI rendering | `RuntimeToolCall.output_preview` |
| verification status items | core | `verification_gates`, `status_items["verification"]` |
| worktree path/branch/diff/closeout | core | `workspace.worktree`, `status_items["worktree*"]` |
| evidence path/status | core | `evidence_refs`, `status_items["evidence"]` |
| policy warnings/decisions | core | `policy_decisions`, `warnings` |
| recovery checkpoint display | core | `status_items["recovery"]` |
| tool focus/selection/expanded state | TUI only | none |
| scroll offsets/click maps/render caches | TUI only | none |
| command palette/dialog/input state | TUI only | none |
| colors/layout/panes | TUI only | none |

The TUI should consume the snapshot for reusable runtime facts while preserving
terminal-specific interaction and rendering state locally.

## Reusable UI guidance

Non-terminal hosts should depend on `imp_core::runtime` types, not `imp-tui`.
Recommended adapter shape:

```rust
pub struct GuiRunViewModel {
    pub title: String,
    pub phase: RuntimePhase,
    pub status_lines: Vec<String>,
}

impl GuiRunViewModel {
    pub fn from_snapshot(snapshot: &RuntimeStateSnapshot) -> Self;
}
```

The GUI can render representative snapshots in tests without launching a live
agent. It should not replay terminal trace JSONL or depend on TUI `App` state.

## RPC and CLI guidance

Existing CLI/RPC `AgentEvent` JSON should remain compatible. Additive runtime
messages can be emitted as:

```json
{
  "type": "runtime_event",
  "event": { "schema_version": 1, "sequence": 1 }
}
```

and/or:

```json
{
  "type": "runtime_state",
  "snapshot": { "schema_version": 1, "phase": "running" }
}
```

Do not remove existing event messages until downstream consumers have a migration
path. Prefer additive runtime payloads and schema-versioned tests.

## Non-goals

- Rewriting the agent loop.
- Removing `AgentEvent` in this epic.
- Moving TUI focus, scroll, pane, or render-cache state into core.
- Making non-terminal hosts depend on `imp-tui`.
- Persisting large artifact contents in snapshots.
- Storing secrets or full sensitive command output beyond existing surfaced
  event data.

## Compatibility tests

Core tests should make schema changes deliberate. Existing focused tests cover:

- `runtime_event_kind_names_are_stable_json_contract`
- `runtime_state_snapshot_replay_fixture_is_stable`
- `runtime_event_roundtrips_through_json`
- `runtime_state_snapshot_roundtrips_through_json`
- `runtime_state_accumulator_reduces_representative_stream`
- `runtime_state_accumulator_tracks_unknown_events_without_corrupting_state`
- `agent_events_convert_to_runtime_events`

Future tests should add golden JSON samples for CLI/RPC runtime payload wrappers
once those transports expose runtime events/snapshots.
