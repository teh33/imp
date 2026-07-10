# Coding eval runner

imp includes a verifier-backed coding evaluation runner for repeatable local comparisons. It prepares an isolated checkout, runs one agent against a pinned task, captures the diff, executes the task verifier, and writes a structured result bundle.

This runner is separate from [eval candidates](eval-candidates.md). Candidates capture interesting failures from normal runs; the eval runner executes curated task specifications.

## Commands

```sh
imp eval list
imp eval validate
imp eval validate <task-id>
imp eval run <task-id> --prepare-only
imp eval run <task-id> --provider <provider> --model <model>
imp eval compare <baseline-result> <candidate-result>
```

The default suite is `evals/coding-agent/tasks`. Override it with `--suite <path>`.

Useful run options:

- `--agent imp|pi` chooses the executable contract.
- `--agent-binary <path>` overrides executable discovery.
- `--provider`, `--model`, and `--thinking` configure an executed run.
- `--compare-pi` runs paired imp/Pi comparisons with identical task settings.
- `--repeat <n>` controls paired repetitions.
- `--results-dir <path>` and `--worktrees-dir <path>` override artifact locations.
- `--json` emits structured command output.

`--prepare-only` validates the task and creates its isolated checkout without running an agent or verifier.

## Task specification

Task files are JSON. The file stem must match `id`.

```json
{
  "id": "rename-method",
  "repo": "https://example.test/project.git",
  "commit": "0123456789abcdef0123456789abcdef01234567",
  "prompt": "Rename the method and update callers.",
  "verifier": "cargo test rename_method",
  "max_turns": 20,
  "timeout_seconds": 1800,
  "verifier_timeout_seconds": 900,
  "expectations": {
    "require_changes": true,
    "allowed_changed_paths": ["src/**", "tests/**"],
    "required_changed_paths": ["src/lib.rs"],
    "max_files_changed": 8
  }
}
```

A task uses either:

- `repo` plus a pinned 40-character `commit`; or
- `fixture`, resolved relative to the task file.

Optional `setup` runs before the agent. `verifier` may contain `<modified-files>`, which expands to shell-escaped changed paths. The run fails rather than invoking that verifier when no tracked paths changed.

Validation rejects moving commit names, unresolved `TBD` verifiers, unsafe task ids, parent traversal in expected paths, zero timeouts, and conflicting change expectations.

## Isolation and artifacts

Remote tasks use a dedicated checkout under `evals/worktrees/<task-id>` by default. Fixture tasks are copied into a fresh local git repository. Existing eval checkouts are reset and cleaned before each remote run; do not point `--worktrees-dir` at a working checkout.

Each run writes under `evals/results/<run-id>/` by default:

```text
prompt.md
agent.stdout.json
agent.stderr.txt
diff.patch
verifier.txt
result.json
```

`result.json` records the source revision, checkout, agent settings and outcome, verifier result, expectation failures, changed paths, line counts, artifact names, and terminal status (`prepared`, `passed`, `failed`, or `blocked`).

`imp eval compare` requires both results to describe the same task. It reports duration and file-count deltas plus pass-to-fail regressions and fail-to-pass improvements.

## Safety notes

- Task sources are pinned before execution.
- Eval work happens in dedicated checkouts, not the current checkout.
- Setup, agent, and verifier processes have bounded timeouts.
- Task verifiers and setup commands execute shell code. Review task files before running an untrusted suite.
- Result artifacts may contain source snippets, model output, and command output. Review them before sharing.
