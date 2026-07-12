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

- `--agent imp|pi|codex|opencode` chooses the executable contract.
- `--agent-binary <path>` overrides executable discovery for individual runs.
- `--provider`, `--model`, and `--thinking` configure an executed run.
- `--compare-pi` is shorthand for a paired imp/Pi comparison.
- `--compare-against pi,codex,opencode` runs one imp candidate and each selected vanilla baseline per repetition.
- `--pi-binary`, `--codex-binary`, and `--opencode-binary` override comparison executables.
- `--repeat <n>` controls comparison repetitions. Execution order rotates deterministically.
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

Changed-path expectations accept shell-style glob patterns. A required pattern must match at least one changed path; every changed path must match an allowed pattern when an allowlist is present.

The verifier command is appended to the execution prompt as the required acceptance command. This gives every compared agent the same environment-specific verification instruction instead of testing whether it guesses aliases such as `python` versus `python3`.

Validation rejects moving commit names, unresolved `TBD` verifiers, unsafe task ids, parent traversal in expected paths, zero timeouts, and conflicting change expectations.

## Isolation and artifacts

Eval worktrees default to an OS temporary directory outside the source repository. Fixture tasks are copied into fresh local Git repositories. Comparison worktrees are namespaced by report id and agent. Existing remote checkouts are reset and cleaned before each run; do not point `--worktrees-dir` at a working checkout.

The runner canonicalizes child cwd and `PWD`. Codex receives an isolated `CODEX_HOME` containing only its authentication link. OpenCode runs with `--pure`, an isolated config directory, external skills disabled, and an explicit checkout directory. A fixture-source guard snapshots local fixtures before each run. If an agent modifies the source fixture rather than its checkout, the run fails, the source is restored, and the violating tree is preserved under `fixture-source-violation/`.

Each run writes under `evals/results/<run-id>/` by default:

```text
prompt.md
agent.stdout.json
agent.stderr.txt
diff.patch
verifier.txt
result.json
```

Comparison runs also write `evals/results/comparisons/<comparison-id>.json`. Schema version 2 stores every agent under one report, records per-repetition execution order, and reports all pass-rate leaders rather than breaking correctness ties with latency.

Token fields use a common convention:

- `raw`: uncached input + cache reads + cache writes + output;
- `effective`: uncached input + cache writes + output;
- `input`: uncached input;
- `output`: generated output plus reasoning when the external CLI reports reasoning separately.

Adapters explicitly declare whether provider-reported input includes cache tokens. This avoids comparing imp/OpenAI's cache-inclusive input shape against Pi or OpenCode's cache-exclusive shape.

`imp eval compare` requires both results to describe the same task. It reports duration and file-count deltas plus pass-to-fail regressions and fail-to-pass improvements.

## Safety notes

- Task sources are pinned before execution and local fixture sources are restored if an agent escapes its checkout.
- Eval work happens in dedicated checkouts, not the current checkout.
- Setup, agent, and verifier processes have bounded timeouts.
- Task verifiers and setup commands execute shell code. Review task files before running an untrusted suite.
- Result artifacts may contain source snippets, model output, and command output. Review them before sharing.
