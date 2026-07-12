# Coding-agent baseline suite

This suite is the fast local baseline for changes to imp's coding loop, task alignment, context selection, and completion policy. It is deliberately independent of workflows.

The task specs use committed fixture directories rather than network repositories. `imp eval` copies each fixture into an isolated Git checkout, commits the starting state, runs the candidate agent, applies deterministic diff constraints, and executes the verifier.

## Tasks

| Task | Behavior under test | Expected edits |
|---|---|---|
| `one-file-fix` | Small focused repair and verification | only `catalog.py` |
| `no-op` | Recognize already-correct behavior | none |
| `preserve-dirty-files` | Preserve an unrelated pre-existing user change | `greeting.py` plus the setup-created `notes.txt` change |

## Commands

```sh
imp eval list --suite evals/coding-agent/tasks
imp eval validate --suite evals/coding-agent/tasks
imp eval run one-file-fix \
  --suite evals/coding-agent/tasks \
  --compare-against pi,codex,opencode \
  --provider openai-codex \
  --model gpt-5.6-sol \
  --repeat 3

# Individual candidates remain available for diagnostics.
imp eval run one-file-fix \
  --suite evals/coding-agent/tasks \
  --agent imp \
  --provider openai-codex \
  --model gpt-5.6-sol

imp eval run one-file-fix \
  --suite evals/coding-agent/tasks \
  --agent pi \
  --provider openai-codex \
  --model gpt-5.6-sol
```

Run each task at least three times with the same binary, provider, model, and thinking level before treating the result as a baseline. Generated results belong under `evals/results/`. Checkouts default to the OS temporary directory outside the source repository. Comparison reports belong under `evals/results/comparisons/`; generated artifacts are ignored by Git.

This first suite is intentionally small. Add regression tasks only when they have a deterministic verifier and a clear changed-path contract.
