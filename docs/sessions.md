# Sessions and evidence

imp stores conversations as durable JSONL sessions and writes inspectable artifacts for agent runs.

Primary implementation areas:

- `crates/imp-core/src/session/`
- `crates/imp-core/src/compaction.rs`
- `crates/imp-core/src/storage.rs`
- `crates/imp-core/src/evidence.rs`
- `crates/imp-core/src/agent/workflow_integration/recipe_runtime.rs`

## Session records

The canonical global session directory is:

```text
~/.imp/sessions/
```

Session entries include headers, user/assistant messages, tool calls and results, usage/cost data, labels, branch metadata, compaction records, and recovery checkpoints. Legacy session roots remain read/migration inputs where supported.

Common CLI controls:

```sh
imp -c                 # continue the most recent session for this cwd
imp -r                 # select a session to resume
imp --session <path>   # open a specific session file
imp --no-session       # ephemeral run
```

## Branching and compaction

Branch metadata preserves alternate conversation paths instead of flattening them into one transcript.

Compaction replaces older model-visible history with a recorded summary while keeping the durable session entries. Compaction is branch-local, so resuming a branch reconstructs the correct visible history.

## Recovery checkpoints

The runtime records checkpoints around provider requests, assistant tool calls, tool execution, and tool results entering context. These records help distinguish:

- a safe provider retry;
- an interrupted call with no side effect;
- a completed side effect whose result needs review;
- a failure that must not be replayed blindly.

## Active run artifacts

Normal agent runs create project-local artifacts under:

```text
<project>/.imp/runs/<run-id>/
```

Depending on the run, the directory can include:

```text
trace.jsonl
workflow-contract.json
evidence.md
policy.jsonl
verify.log
diff.patch
worktree/
eval-candidates/
```

`trace.jsonl` is structured runtime evidence. `evidence.md` is the human summary. The workflow-contract snapshot captures role, autonomy, workspace scope, trust policy, and verification expectations for that run.

The older `imp_core::run_evidence` HTML/index implementation and `imp evidence list/latest` commands still exist, but they are not the source of the project-local evidence packet written by the current agent loop. Treat those commands as compatibility/partial until the two evidence paths are unified.

## Workflow evidence

Durable workflows keep their own state under `.imp/workflows/<id>/`:

- `workflow.yaml` for the contract and current status;
- `events.jsonl` for append-only mutations;
- `results.md` for closeout;
- `artifacts/` for supporting files.

Workflow records complement run artifacts; they do not replace the conversation session or trace.

## Outcomes

Common terminal outcomes are:

```text
DONE
DONE_WITH_CONCERNS
BLOCKED
NEEDS_CONTEXT
FAILED
CANCELLED
```

Use `DONE_WITH_CONCERNS` only when useful work completed but a material limitation remains. Failed or concern-bearing closeout can produce an [eval candidate](eval-candidates.md) under the run artifacts.

## Privacy

Sessions, traces, evidence, verifier logs, and eval candidates can contain prompts, source paths, command output, or model output. They remain local by default, but should be reviewed and redacted before sharing.
