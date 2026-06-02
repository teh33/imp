# Workflow Readiness and Parallel Branch Suggestions

Status: implemented 2026-06-02

## Summary

This note proposed a small, non-rearchitectural improvement slice for imp-native workflows, implemented on 2026-06-02:

1. richer derived readiness reasons;
2. clearer blocked/no-runnable explanations;
3. safe subagent batching for parallel runnable branches.

The intent is to make workflows better at agent self-orchestration without turning them into a project-management graph, scheduler, or second mana. Workflows should remain compact runtime contracts: they explain what is runnable, why other work is blocked, and which independent branches can be delegated safely.

## Non-goals

This proposal intentionally does **not** add:

- task-board semantics;
- durable worker leases or step claims;
- priority scoring;
- retry policy;
- persisted attempt history;
- project-wide indexing;
- archive/reopen lifecycle;
- a general multi-agent scheduler.

Those features either belong in mana/project graph tooling or should wait until workflows show concrete pain.

## Prior behavior

Before this implementation, the workflow tool already had a useful base:

- `workflow run` validates a workflow, executes pending executable checks for the next runnable step, or returns an action/subagent contract.
- `next_runnable_steps` derives runnable steps from step status, dependencies, and worker existence.
- `NoRunnableSteps` reports blocked/pending steps with basic reasons.
- `run_mode = subagents` can produce a bounded subagent action for a runnable step.

The limitation addressed by this implementation was that workflow execution mostly thought in terms of a single next step. When no step was runnable, the explanation was shallow. When multiple branches were runnable, subagent mode still selected one branch instead of returning a safe parallel wave.

## Proposal 1: derived readiness reasons

Introduce an internal readiness calculation that explains every step, not just the next step.

This should be derived from existing workflow state. It does not require a schema change.

Conceptual shape:

```rust
pub struct WorkflowStepReadiness {
    pub step: String,
    pub status: String,
    pub state: WorkflowReadinessState,
    pub reasons: Vec<WorkflowReadinessReason>,
}

pub enum WorkflowReadinessState {
    Runnable,
    Waiting,
    Blocked,
    Terminal,
}

pub struct WorkflowReadinessReason {
    pub kind: WorkflowReadinessReasonKind,
    pub message: String,
    pub subject: Option<String>,
}
```

Useful reason kinds implemented in the first slice:

- `dependency_not_ready`
- `dependency_missing`
- `worker_missing`
- `status_not_runnable`
- `check_pending`
- `check_failed`
- `check_blocked`

Reason kinds that remain possible future additions when the engine needs them:

- `approval_pending`
- `artifact_missing`
- `action_missing`
- `child_workflow_missing`
- `child_workflow_incomplete`
- `validation_blocked`

`next_runnable_steps` can remain as a compatibility helper implemented on top of the richer readiness model.

### Minimal behavior

A step is runnable when:

- status is `todo` or `ready`;
- all `depends_on` steps exist and are terminal-success;
- assigned worker exists, if specified.

The first implementation deliberately keeps action-contract and child-workflow issues outside the readiness gate so existing command-check orchestration and missing-action-contract behavior remain compatible. Those issues are still surfaced by the workflow tool when a runnable step is selected.

A terminal step is one of:

- `done`
- `done_with_concerns`
- `skipped`
- `failed`
- `blocked` when treated as a terminal non-success for closeout/blocker reporting

The exact terminal-success helper should preserve existing behavior: dependencies are satisfied by `done` or `done_with_concerns`.

## Proposal 2: better blocked/no-runnable explanations

Use readiness output to improve `WorkflowNextAction::NoRunnableSteps` rendering and JSON details.

Current style:

```text
No runnable workflow steps.
Blocked/pending steps:
- verify [todo]: dependency `build` is active
```

Suggested style:

```text
No runnable workflow steps.

Readiness summary:
- runnable: 0
- waiting: 2
- blocked: 1
- terminal: 4

Blocked/pending steps:
- verify [todo]
  - dependency `build` is active
  - check `unit_tests` is pending
- closeout [todo]
  - closeout requirement `final_claims_validated` is pending
```

The model-facing JSON should include structured reasons so the agent can decide whether to continue, ask the user, launch a subagent, or report a blocker without parsing prose.

This is the highest-leverage change because it improves stuck-workflow behavior without adding any new workflow syntax.

## Proposal 3: subagent batching for parallel runnable branches

When `workflow run` is called with `run_mode = subagents`, the tool should return a safe batch of independent runnable action steps instead of only the first runnable step.

Conceptual next action:

```rust
WorkflowNextAction::SubagentBatch {
    assignments: Vec<WorkflowSubagentBatchAssignment>,
    held_back: Vec<WorkflowHeldBackStep>,
}
```

Rendered example:

```text
Workflow recommends 2 parallel subagent actions.

Launchable:
- cli_changes [build]: CLI implementer
  - writes: crates/imp-cli/**
- core_changes [build]: Core implementer
  - writes: crates/imp-core/**

Held back:
- docs_update: write scope overlaps with cli_changes
```

### Safe batching rules

Keep this deliberately small:

1. only batch in `run_mode = subagents`;
2. only consider currently runnable steps;
3. only include steps with explicit action contracts;
4. do not batch two steps where one depends on the other;
5. do not batch steps with overlapping write scopes unless future workflow settings explicitly allow it;
6. cap batch size with a conservative default, such as 3 or 4;
7. include held-back reasons instead of silently ignoring runnable steps.

This is not a scheduler. It is a runnable-wave return value.

### Command checks interaction

Preserve existing command-check behavior:

1. `workflow run` may first execute pending executable checks for runnable command-check steps;
2. reload the workflow after those updates;
3. compute the runnable action wave;
4. in subagent mode, return a batch if more than one safe action step remains.

This keeps existing check orchestration intact while making parallel action branches useful.

### Write-scope overlap

Start conservative. Treat write scopes as overlapping when:

- two exact paths match;
- one path is a parent/prefix of the other;
- either scope is broad, such as `.`, `**`, `crates/**`, or an unparseable glob that could include the other.

Conservative false positives are acceptable. The goal is to avoid unsafe parallel edits, not to build a perfect glob engine.

## Authored blocked reasons are deferred

Authored blocked reasons may be useful later, for blockers the engine cannot infer:

```yaml
steps:
  migration:
    status: blocked
    blocked:
      kind: user_decision
      reason: Need approval before changing durable workflow schema.
```

Do not add this in the first slice. Derived readiness should come first. If agents still need to record human/context blockers, add a minimal schema field later.

## Step claims are deferred

Current workflow schema does not appear to have step-level ownership claims or leases. It has result claims, worker assignment, and step status, but not `claimed_by` / `claimed_at`.

Do not add claims in this slice. Subagent batching can avoid duplicate launches because the parent workflow tool returns one bounded batch. Claims become useful only if independent sessions/processes routinely race to work the same workflow.

## Attempt history is deferred

Attempt history can be derived from `events.jsonl` if needed. Persisted attempt fields would duplicate event state and likely lead to retry-policy complexity.

If this becomes useful, start with an event-derived summary in `workflow show` or `workflow run` details:

```text
verify: 3 attempts, last failed because cargo test exited 101
```

Do not add persisted attempts in this slice.

## Implementation notes

Implemented artifacts:

- `.imp/workflows/workflow-orchestration-clarity/results.md`
- `.imp/workflows/workflow-readiness-reasons/results.md`
- `.imp/workflows/workflow-blocked-explanations/results.md`
- `.imp/workflows/workflow-subagent-branch-batching/results.md`

Implemented code areas:

- `crates/imp-core/src/workflow/schema.rs`
- `crates/imp-core/src/tools/workflow.rs`
- `crates/imp-core/src/tools/workflow/tests.rs`

The shipped implementation kept the scope additive:

- no workflow YAML schema changes;
- no claims or leases;
- no persisted attempt history;
- no priority/scoring scheduler;
- no project graph behavior;
- no new dependencies.

## Implementation slices

### Slice 1: readiness engine

- Add a derived readiness helper in `crates/imp-core/src/workflow/`.
- Keep `next_runnable_steps` behavior-compatible.
- Replace or augment `blocked_steps` with structured readiness reasons.
- Add focused tests for dependency, missing worker, failed/pending checks, terminal states, and missing action contract.

### Slice 2: better run output

- Update `WorkflowNextAction::NoRunnableSteps` details and rendering.
- Include readiness summary counts.
- Ensure JSON details include structured reasons.
- Preserve existing text expectations where possible, but improve stale fallback wording.

### Slice 3: subagent batch

- Add a `SubagentBatch` next-action variant.
- In subagent mode, collect a safe runnable wave.
- Add conservative write-scope conflict detection.
- Return held-back steps with reasons.
- Keep single-step `SubagentAction` behavior when only one safe subagent is launchable.

## Success criteria

This improvement is successful when:

- an agent can tell why every non-terminal workflow step is not runnable;
- `NoRunnableSteps` rarely needs vague fallback explanations;
- parallel runnable branches produce a bounded subagent batch in subagent mode;
- unsafe overlapping branches are held back with clear reasons;
- no new YAML fields are required for the core behavior;
- workflows still feel like compact execution contracts, not a task-board system.

## Verification suggestions

Narrow tests should cover:

- readiness for a simple linear workflow;
- readiness for a missing dependency;
- readiness for active/failed dependency;
- readiness for missing worker;
- `NoRunnableSteps` rendering with multiple reason kinds;
- subagent batch with independent write scopes;
- subagent batch holding back overlapping write scopes;
- compatibility of `next_runnable_steps` ordering and dependency behavior.

Run at least:

```sh
cargo test -p imp-core workflow
cargo test -p imp-cli workflow_cli
cargo fmt --check
```

Use broader workspace checks only if shared workflow API changes affect downstream crates.
