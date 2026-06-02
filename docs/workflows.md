# Workflows

imp workflows are local project artifacts for planned, multi-step work. They keep the plan, execution state, checks, prototype results, events, and closeout notes in files under the project.

Primary implementation areas:

- `crates/imp-core/src/workflow/schema.rs`
- `crates/imp-core/src/tools/workflow.rs`
- `crates/imp-core/src/workflow/controller.rs`
- `crates/imp-core/src/workflow/child_workflow.rs`

## Layout

```text
.imp/workflows/<id>/
├── workflow.yaml
├── events.jsonl
├── results.md
└── artifacts/
```

`workflow.yaml` is the contract. `events.jsonl` is append-only update history. `results.md` is the human-readable closeout record. `artifacts/` holds plans, outputs, fixture files, review notes, or other supporting evidence.

## Schema

Common top-level fields:

- `schema`
- `id`
- `title`
- `status`
- `kind`
- `parent`
- `settings`
- `spec`
- `context`
- `steps`
- `prototypes`
- `checks`
- `workers`
- `results`
- `closeout`

`spec.acceptance` records user-facing completion criteria. `steps` record the work sequence. `checks` record the verification gates or artifacts that prove a step. `closeout` records terminal status requirements.

## Status values

Workflow, step, and check status values are schema-validated. The valid sets differ by object type:

- workflow statuses include `planned`, `active`, `done`, `done_with_concerns`, `blocked`, and `needs_context`;
- step statuses include `todo`, `ready`, `active`, `waiting`, `blocked`, `done`, `done_with_concerns`, `skipped`, and `failed`;
- check statuses include `pending`, `passed`, `failed`, `blocked`, and `skipped`.

Invalid status updates are rejected before `workflow.yaml` is written. Oversized workflow YAML is rejected before parsing. Successful updates validate the prospective workflow, open/preflight `events.jsonl`, replace `workflow.yaml`, then append the event; this is safer than mutating state without an event sink, but it is not a full crash-proof two-file transaction.

## Tool actions

The native `workflow` tool supports:

```text
list
show
validate
run
complete_step
update
```

`validate` parses and checks workflow structure. `run` selects the next runnable step. If that step has pending command checks, `run` executes those checks in the project root, updates each check to `passed` or `failed`, updates the step to `done` or `failed`, appends events, reconciles acceptance/closeout state where possible, and returns a run summary. If the runnable step needs agent judgment, `run` returns an action contract for the main agent or, when requested with `run_mode="subagents"`, a bounded subagent contract. Run outputs include structured metadata such as `action`, workflow `id`, `status`, execution mode, and `next_action` so the agent loop can continue workflow execution instead of stopping after one tool call.

`complete_step` is the ergonomic closeout action for agent-actionable work. It marks a step `done`, marks the step's attached checks `passed`, reconciles acceptance criteria and workflow closeout state, appends events, and validates the resulting workflow before writing. Agent action contracts returned by `run` tell the agent to call `complete_step` when the contracted work is finished.

`update` mutates an allowed status path and appends an event. It remains useful for explicit status repair or blocker reporting, but routine successful step completion should prefer `complete_step`.

## Lifecycle

```text
inspect → validate → run → update → events → prototype/verify → review → closeout
```

A typical agent loop is:

1. inspect workflow context
2. run `workflow validate`
3. run `workflow run` to select or execute the next step
4. do any non-executable work requested by the run output's action contract
5. call `workflow complete_step` when the contracted work is complete, or report a concrete blocker with a status update when it is not
6. run `workflow run` again to continue orchestration
7. verify command/artifact evidence
8. write `results.md`
9. close the workflow with a terminal status

When the user asks to “run the workflow”, the agent should repeat this loop until the workflow is complete, no runnable work remains, validation/dependency state blocks progress, a failed check needs recovery, or a user decision/policy denial is required.

## Events

Each successful update appends a JSON line to `events.jsonl`. Executable `run` actions append events for check and step status changes. `complete_step` appends events for the completed step, attached checks, and any reconciled acceptance or workflow status updates. Events include the action, path, value, reason, and timestamp. This makes workflow progress inspectable outside the chat transcript.

## Prototyping

Prototype work belongs in the workflow when an implementation decision needs evidence. A prototype entry should state:

- question
- hypothesis
- status
- criteria
- evidence required
- follow-up work

Prototype artifacts should be disposable unless explicitly promoted into production code or documentation.

## Verification and closeout

Checks can represent commands, artifacts, context review, aggregate gates, or manual review. Command checks with `status: pending` and a `command` attached to the next runnable step are executable through `workflow run`; imp records the check result and step outcome in `workflow.yaml` and `events.jsonl`. Closeout should not rely only on a narrative claim; it should point to completed checks and a results artifact.

Terminal outcomes used by imp work include:

```text
DONE
DONE_WITH_CONCERNS
BLOCKED
NEEDS_CONTEXT
```

## Current limitations

- Storage is local and file-backed.
- API-addressable workflows are planned, not shipped.
- Direct mutation hardening is still evolving; workflow update should continue to get stricter around premature closeout states.
