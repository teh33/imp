# Workflow-first imp UX migration guide

Workflow-first imp should feel like the same imp to existing users: open the TUI,
ask for help, review the answer. The new runtime adds stronger structure around
longer or more autonomous work, but it should appear only when it helps you
understand, control, or review a run.

Use this guide progressively:

1. **Start normally** — no workflow concepts required for normal TUI use.
2. **Notice better status** — imp records runtime state, policy decisions,
   evidence, and verification behind the scenes.
3. **Add quick controls** — autonomy modes, verification commands, and closeout
   choices.
4. **Review and audit** — evidence packets, traces, workflow ledger, runtime state.
5. **Use advanced workflows** — worktree-auto, roles, delegation, and CI/headless output.

## 1. Existing usage still works

You can keep using imp the way you already do:

```sh
imp
imp "fix the parser bug"
imp -p "summarize this repo"
imp chat
imp run 394.10.4
```

The TUI remains the default interactive experience. Direct natural-language tasks
still work. Routine work does not require manually creating workflow tasks, choosing
roles, or reading evidence files.

What changes is mostly invisible: imp has more durable runtime state available
for status, policy checks, evidence, verification, and recovery when the task
needs it.

## 2. What workflow-first changes invisibly

Workflow-first imp is better at answering review questions:

- What did imp do?
- Which tools ran?
- Why was a tool allowed, denied, or gated?
- What files or artifacts matter?
- What verification passed or failed?
- What remains blocked or concerning?

The implementation details are runtime events/snapshots, trace artifacts,
evidence packets, ReferenceMonitor policy records, verification gates, and workflow
ledger notes. You do not need to workflowge those pieces for small tasks; they are
there when a run becomes long, autonomous, reviewable, or recoverable.

## 3. TUI-first workflow status

The TUI remains chat-first. Workflow-first state shows up as supporting context:

- phase/model/tool status
- tool execution cards
- warnings and policy messages
- verification summaries
- worktree path/diff/closeout status
- evidence links
- final closeout status

The goal is not to turn the TUI into a project-workflowgement dashboard. The chat is
still primary; workflow state helps explain and review what happened.

## 4. Autonomy modes and quick examples

Autonomy controls how much imp can do without stopping for approval.

```sh
imp --autonomy safe "inspect this failure"
imp --autonomy local-auto "fix the failing test"
imp --autonomy allow-all-local "format and test the repo"
imp --autonomy allow-all "run the full migration task"
imp --autonomy ci --verify "cargo test" "check this branch"
```

Practical guidance:

- `safe` / `suggest`: conservative inspection and low-risk work.
- `local-auto`: autonomous implementation in the current workspace with policy
  guardrails.
- `allow-all-local`: trusted local automation with fewer interruptions.
- `allow-all`: broad authority; use only when you really trust the task and
  environment.
- `ci`: non-interactive verification/automation contexts.

You can still rely on the normal TUI approval flow. Advanced autonomy is optional.

## 5. Evidence packets and trace artifacts

Reviewable runs can write artifacts under `.imp/runs/<run-id>/`.

Common artifacts:

- evidence packet
- trace JSONL
- tool outputs
- verification output
- worktree status/diff/patch files

Evidence packets summarize the run for humans: what changed, what was verified,
what failed, what policy decisions mattered, and what remains. You usually do not
need to inspect evidence for small interactive tasks, but it is useful for code
review, handoff, debugging, CI, and autonomous-run audit.

## 6. Workflow workflow ledger

Workflow is imp’s durable workflow ledger. It is useful when work has acceptance
criteria, dependencies, decisions, blockers, verification gates, or handoff notes.
It is not required for every prompt.

Examples:

```sh
imp run 12.1
imp "continue the 394 epic"
imp "work on the next open task"
```

Use workflow when the work should survive beyond one chat turn. Ignore it for simple
questions, small edits, or exploratory conversations.

## 7. Verification gates and closeout statuses

Verification gates make “done” explicit:

```sh
imp --verify "cargo test -p imp-core" "fix the runtime bug"
imp --autonomy worktree-auto --verify "cargo test" "implement the change"
```

Closeout statuses should be clear:

- `DONE`: completed and verified.
- `DONE_WITH_CONCERNS`: useful result, but notable issues remain.
- `BLOCKED`: imp cannot proceed without a decision or external fix.
- `NEEDS_CONTEXT`: imp needs more information.

If verification fails, imp should say so. Failed tests, compiler errors, missing
evidence, or unverified claims should not be presented as success.

## 8. Worktree-auto UX

`worktree-auto` isolates autonomous implementation in a separate git worktree:

```sh
imp --autonomy worktree-auto "implement this refactor"
```

imp captures worktree status, diff stats, patch files, and metadata under the run
artifacts. Closeout commands use the saved metadata file:

```sh
imp worktree status .imp/runs/<run-id>/worktree/worktree-metadata.json
imp worktree apply .imp/runs/<run-id>/worktree/worktree-metadata.json
imp worktree keep .imp/runs/<run-id>/worktree/worktree-metadata.json
imp worktree discard .imp/runs/<run-id>/worktree/worktree-metadata.json
```

Use:

- `status` to inspect the isolated result.
- `apply` to conservatively apply the patch to the main checkout.
- `keep` to leave the worktree for manual review.
- `discard` to remove the worktree and branch.

See [`worktree-auto.md`](worktree-auto.md) for details.

## 9. Roles and child workflow delegation

Roles and delegation are advanced workflow concepts for larger work:

- planner
- coder
- verifier
- reviewer
- child workflow delegation

These concepts should appear only when useful. Routine users should not need to
choose roles manually. Where role/delegation behavior is still evolving, imp
should describe it as planned or advanced rather than implying every role flow is
fully automatic.

## 10. CI/headless workflows

Headless workflows are useful when imp runs without an interactive TUI:

```sh
imp --autonomy ci --verify "cargo test" "check this branch"
imp --runtime-json --output json "summarize verification"
```

Expect stricter policy behavior: no interactive approval prompts, clearer failure
reporting, JSON/runtime state output when requested, and evidence artifacts for
review.

## 11. FAQ and troubleshooting

### Do I need to learn workflow?

No. Use workflow when work is durable, multi-step, blocked, delegated, or needs
handoff. For normal questions and small edits, just ask imp.

### Why did imp ask for approval?

A policy check saw a tool action that might write files, access external state,
use network/secrets, or otherwise exceed the current autonomy mode.

### Where did my evidence packet go?

Look under `.imp/runs/<run-id>/`. The TUI and final answer should link important
artifacts when a run writes evidence.

### What does `DONE_WITH_CONCERNS` mean?

The task produced a useful result, but something remains worth noting: partial
verification, unrelated failing tests, warnings, cleanup needed, or a risky
follow-up.

### How do I keep/apply/discard worktree-auto changes?

Use the metadata file saved under `.imp/runs/<run-id>/worktree/`:

```sh
imp worktree status .imp/runs/<run-id>/worktree/worktree-metadata.json
imp worktree apply .imp/runs/<run-id>/worktree/worktree-metadata.json
imp worktree discard .imp/runs/<run-id>/worktree/worktree-metadata.json
```

### How do I run imp the old way?

Use the same commands as before:

```sh
imp
imp -p "quick question"
imp chat
```

Workflow-first features should stay out of the way until they are useful.

## README link strategy

README should stay short. Add a compact link such as:

> New to workflow-first imp? See `docs/workflow-first-ux.md` for the migration
> guide covering autonomy modes, evidence, verification, worktrees, extensions,
> and CI/headless use.

Do not duplicate the full guide in README.
