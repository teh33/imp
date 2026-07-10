# Runtime event and state API

`imp_core::runtime` defines shared, frontend-neutral runtime facts for CLI, TUI, RPC, tests/replay, and the experimental GUI.

The core surface is:

1. `RuntimeEvent`, a schema-versioned event;
2. `RuntimeStateSnapshot`, current reduced state;
3. `RuntimeStateAccumulator`, a deterministic event reducer.

`AgentEvent` remains the agent loop's compatibility stream. Runtime events adapt that stream rather than replacing it.

## Versioning

```rust
pub const RUNTIME_SCHEMA_VERSION: u32 = 1;
```

Both events and snapshots carry `schema_version`. Breaking serialized changes require an explicit version bump and compatibility-test updates.

## Events

```rust
pub struct RuntimeEvent {
    pub schema_version: u32,
    pub run_id: String,
    pub sequence: u64,
    pub timestamp_ms: Option<u64>,
    pub kind: RuntimeEventKind,
}
```

Current event kinds cover:

- agent/turn/message lifecycle;
- tool start, output, and completion;
- approval pending/resolved;
- policy decisions;
- workflow-controller updates;
- verification and evidence updates;
- child-workflow updates;
- worktree updates;
- legacy mana/workflow references;
- warnings, errors, timing, recovery checkpoints, and unknown events.

`Unknown` preserves forward/foreign event names without corrupting accumulated state.

## Agent-event adapter

```rust
impl AgentEvent {
    pub fn to_runtime_event(
        &self,
        run_id: impl Into<String>,
        sequence: u64,
    ) -> RuntimeEvent;
}
```

The adapter maps lifecycle, turns, messages, tools, warnings/errors, timing, recovery, workflow-controller state, verification, evidence, worktree metadata, and policy checks into typed runtime payloads.

## Snapshot

`RuntimeStateSnapshot` contains:

- schema version;
- run/model summary;
- autonomy and workspace/worktree state;
- phase and terminal status;
- active/completed tools;
- pending approvals and policy decisions;
- verification gates and evidence refs;
- child workflows;
- legacy workflow refs;
- warnings/errors;
- compact `status_items` for presentation adapters.

Presentation-only state does not belong here. Scroll positions, selected tools, pane layout, dialog state, colors, and render caches stay in the frontend.

## Accumulator

```rust
pub struct RuntimeStateAccumulator { /* private */ }

impl RuntimeStateAccumulator {
    pub fn new(run_id: impl Into<String>) -> Self;
    pub fn from_snapshot(snapshot: RuntimeStateSnapshot) -> Self;
    pub fn apply(&mut self, event: &RuntimeEvent);
    pub fn snapshot(&self) -> RuntimeStateSnapshot;
}
```

The reducer is deterministic and side-effect free. It upserts tools, approvals, gates, artifacts, child workflows, and worktree state while maintaining phase and compact status labels.

## Consumers

- The TUI should consume shared runtime facts while keeping terminal interaction local.
- New RPC consumers can request `--runtime-json`; legacy fields remain for compatibility.
- The experimental GUI depends on `imp_core::runtime`, not `imp-tui`.
- Tests can build representative snapshots without launching a provider.

Example additive RPC wrappers:

```json
{"type":"runtime_event","event":{"schema_version":1,"sequence":1}}
```

```json
{"type":"runtime_state","snapshot":{"schema_version":1,"phase":"running"}}
```

Consumers should ignore unknown fields and event names where possible.

## Non-goals

- Rewriting the agent loop.
- Removing `AgentEvent` without a migration.
- Moving frontend interaction state into core.
- Persisting full artifact contents in snapshots.
- Storing secret values or unbounded command output.

## Compatibility coverage

Focused tests cover stable kind names, JSON round-trips, representative stream reduction, unknown events, and `AgentEvent` conversion. Any serialized-contract change should update those tests deliberately.
