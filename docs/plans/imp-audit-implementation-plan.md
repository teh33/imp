# imp audit implementation plan

Status: proposed execution plan for the imp-native audit tool described in `docs/design/imp-audit-tool.md`.

## Target outcome

Ship an imp-native audit capability that gives agents compact, actionable repo-quality signals and can optionally coordinate bounded repairs.

The audit system should be able to:

- discover repo-native checks and optional external tools;
- run checks with bounded process control and artifact capture;
- report missing tools as skipped/blocked with install options;
- summarize findings with new/existing/fixed baseline context;
- run high-signal native agent-quality checks without external dependencies;
- expose optional, explicit install flows for missing programs;
- plan repair clusters and later delegate safe clusters to subagents.

## Non-goals

- Do not reimplement large mature tools such as Semgrep, CodeQL, Sonar, Warden, or full language linters.
- Do not install package-manager dependencies during normal audit runs.
- Do not run network/package-manager commands from hooks.
- Do not make LLM-backed checks part of the default quick profile.
- Do not auto-fix or spawn subagents without explicit configuration/user intent.

## Architecture decision

Create a new internal crate:

```text
crates/imp-audit
```

Use thin integration layers:

```text
crates/imp-core/src/tools/audit.rs       # model-facing tool wrapper
crates/imp-cli                           # optional `imp audit ...` CLI later
crates/imp-tui                           # optional install/doctor UI later
```

`imp-audit` owns pure audit logic. `imp-core` owns agent/session/workflow integration and subagent launching.

## Milestone 0: research and contract freeze

Deliverables:

- finalize `docs/design/imp-audit-tool.md`;
- decide tool name: recommended `audit` for user/model surface, crate `imp-audit`;
- decide v1 parser set;
- decide v1 native rules;
- define initial `.imp/audit.toml` schema;
- define install-offer safety policy.

Exit criteria:

- design doc reviewed;
- crate boundary accepted;
- install UX policy accepted;
- v1 scope explicitly excludes auto-fix/subagent execution.

## Milestone 1: crate skeleton and core data model

Files/modules:

```text
crates/imp-audit/Cargo.toml
crates/imp-audit/src/lib.rs
crates/imp-audit/src/model.rs
crates/imp-audit/src/summarize.rs
crates/imp-audit/src/error.rs
```

Implement:

- `AuditProfile`, `AuditCheck`, `AuditReport`, `AuditFinding`;
- severities: error, warning, info, hint, unknown;
- check outcomes: passed, failed, skipped, blocked, timed_out;
- finding source: native, command, external_tool, parser, agent_safety;
- compact summary renderer;
- JSON-serializable report shape.

Acceptance tests:

- serializes/deserializes report shape;
- summarizes passed/failed/skipped/blocked checks;
- truncates finding lists to `max_findings` while preserving counts.

## Milestone 2: discovery, scope, and command runner

Files/modules:

```text
crates/imp-audit/src/discover.rs
crates/imp-audit/src/scope.rs
crates/imp-audit/src/runner.rs
crates/imp-audit/src/artifacts.rs
```

Implement:

- repo root detection;
- scopes: changed, staged, workspace, files;
- command execution with timeout, stdin null, stdout/stderr capture;
- process-group cleanup on timeout;
- artifact directory per run;
- output truncation with full logs persisted;
- generic command failure summarization.

Discovery v1:

- Rust: `Cargo.toml` -> cargo fmt/check/test/clippy candidates;
- JS/TS: `package.json` scripts for lint/typecheck/test;
- Python: `pyproject.toml`, `ruff.toml`, pytest config;
- Go: `go.mod` -> gofmt/go test/go vet;
- generic: `Makefile`, `Justfile`, `.pre-commit-config.yaml` as candidates, not auto-required.

Acceptance tests:

- changed/staged scope against fixture repos;
- timeout kills child process groups;
- missing command returns structured skipped/blocked outcome;
- artifact paths are created and log truncation is reported.

## Milestone 3: `.imp/audit.toml` config and profiles

Files/modules:

```text
crates/imp-audit/src/config.rs
crates/imp-audit/src/requirements.rs
```

Implement:

- config loading from `.imp/audit.toml`;
- merge discovered checks with configured checks;
- profiles: quick, full, quality, ai_quality, security, deps, secrets, fuzz;
- check fields: id, kind, command, parser, paths, optional, requires, timeout;
- requirements/install recipe model;
- validation with actionable errors.

Acceptance tests:

- loads example config;
- rejects duplicate check ids;
- rejects install recipe without exact command tokens;
- optional missing requirement becomes skipped;
- required missing requirement becomes blocked.

## Milestone 4: missing-tool UX and explicit install flow

This milestone directly answers how imp should let users know about missing programs and possibly offer installation.

Behavior model:

```text
audit discover:
  reports available/missing requirements and install recipes.

audit run:
  never installs;
  skips optional missing tools;
  blocks required missing tools;
  includes compact install hints.

audit doctor:
  prints a human-readable readiness table;
  supports JSON for scripts.

audit install:
  high-risk explicit action;
  requires exact check/requirement id + recipe id;
  requires confirmation in UI or `--yes` in CLI;
  records artifacts.
```

Interactive TUI/headless rules:

- TUI may ask via the existing user-confirm/select UI.
- Headless mode may only install with `--yes` and exact recipe id.
- Model-facing tool calls should not silently install. If exposed at all, install must be a separate mutating action routed through policy and user confirmation.
- Hooks and subagent repair must not perform installs.

Install prompt should include:

- missing tool name and why it helps;
- exact command;
- working directory;
- whether it mutates project files;
- whether it requires network;
- likely files affected, such as `package.json`, lockfiles, or `pyproject.toml`;
- skip option;
- disable-check option.

Example output:

```text
Skipped optional check `aislop`: command `aislop` was not found.

Install options:
1. npm install --save-dev aislop        project dev dependency; mutates package files; network
2. brew install scanaislop/tap/aislop   user/global install; network
3. Skip for now
4. Disable check `aislop` in .imp/audit.toml
```

Acceptance tests:

- `audit run` never invokes installer recipes;
- `audit doctor --json` lists missing requirements and recipes;
- install action refuses missing `--yes` in non-interactive mode;
- project-mutating install warns when package files are dirty;
- installer output is captured and redacted/truncated as artifact.

## Milestone 5: parsers and external adapters

Files/modules:

```text
crates/imp-audit/src/adapters/mod.rs
crates/imp-audit/src/adapters/generic.rs
crates/imp-audit/src/adapters/sarif.rs
crates/imp-audit/src/adapters/cargo.rs
crates/imp-audit/src/adapters/eslint.rs
crates/imp-audit/src/adapters/ruff.rs
crates/imp-audit/src/adapters/gitleaks.rs
crates/imp-audit/src/adapters/semgrep.rs
crates/imp-audit/src/adapters/osv.rs
crates/imp-audit/src/adapters/aislop.rs
```

V1 parsers:

- generic `path:line:column` matcher;
- SARIF ingest;
- cargo/clippy JSON or stderr fallback;
- Ruff JSON;
- ESLint/Biome JSON;
- Gitleaks JSON;
- Semgrep JSON/SARIF;
- OSV JSON;
- aislop JSON when installed.

Acceptance tests:

- parser fixtures produce normalized `AuditFinding`s;
- unknown parser output degrades to command summary rather than failing the whole audit;
- SARIF parser preserves rule id, severity, path, line, message.

## Milestone 6: baseline and suppressions

Files/modules:

```text
crates/imp-audit/src/baseline.rs
crates/imp-audit/src/suppressions.rs
```

Implement:

- finding fingerprint: source, rule id, normalized path, line bucket, message hash;
- baseline load/save under `.imp/audit/baseline.json` or configured path;
- new/existing/fixed finding classification;
- inline suppressions with rule id and reason;
- config suppressions by path/rule;
- required reason for high/critical suppressions.

Acceptance tests:

- existing finding is hidden from blocker summary by default;
- new finding in changed file is primary blocker;
- fixed finding count is reported;
- suppression without reason is rejected for high severity.

## Milestone 7: native agent-quality rules

Files/modules:

```text
crates/imp-audit/src/native/mod.rs
crates/imp-audit/src/native/rules.rs
crates/imp-audit/src/native/text.rs
crates/imp-audit/src/native/rust.rs
crates/imp-audit/src/native/typescript.rs
crates/imp-audit/src/native/python.rs
crates/imp-audit/src/native/agent_safety.rs
```

V1 native rules:

- placeholder implementations: `todo!`, `unimplemented!`, `pass`, stub returns;
- swallowed errors: empty catch/except, ignored result patterns;
- debug leftovers: `dbg!`, `console.log`, raw print in likely library code;
- broad casts/suppressions: `as any`, unreasoned `type: ignore`;
- narrative/hedging comments;
- oversized functions/files and deep nesting;
- unsafe shell/process code without timeout/cancellation;
- hook/config changes that expand execution without explicit safeguards;
- dependency-file changes without matching dependency audit check.

Acceptance tests:

- fixture per rule with positive/negative cases;
- rules default to changed files for quick profile;
- workspace-wide findings are counted but not over-promoted in quick summary.

## Milestone 8: imp-core tool integration

Files/modules:

```text
crates/imp-core/src/tools/audit.rs
crates/imp-core/src/tools/mod.rs
```

Implement model-facing tool:

```json
{
  "action": "discover | run | doctor | explain | baseline | plan_repair",
  "profile": "quick | full | quality | ai_quality | security | deps | secrets | fuzz",
  "scope": "changed | staged | workspace | files",
  "paths": [],
  "base": "HEAD",
  "max_findings": 20,
  "timeout_seconds": 120
}
```

V1 should omit mutating install and repair execution from the model-facing tool unless policy/user confirmation support is complete.

Acceptance tests:

- tool schema is stable;
- discover returns compact availability table;
- run returns structured details plus compact text;
- missing optional external tool is skipped with install hint;
- missing required external tool is blocked with install hint.

## Milestone 9: hooks, workflows, and closeout

Implement:

- audit hook config parsing from `.imp/audit.toml`;
- conservative `on_agent_end` quick audit option;
- no after-file-write hook enabled by default;
- verification gate integration for audit profile runs;
- workflow artifact references for audit logs/reports.

Acceptance tests:

- no audit hook runs without explicit config;
- configured on-agent-end hook runs quick audit;
- hook never installs missing tools;
- failed required audit check blocks workflow closeout when configured as required.

## Milestone 10: repair planning and subagent orchestration

Files/modules:

```text
crates/imp-audit/src/repair_plan.rs
crates/imp-audit/src/orchestration/cluster.rs
crates/imp-audit/src/orchestration/subagent_contract.rs
```

Implement first as planning only:

- cluster findings by path/module/rule/verification command;
- classify clusters as auto_fixable, agent_fixable, requires_main_agent, requires_user_decision, ignore_existing;
- generate bounded subagent contracts;
- expose `plan_repair` action.

Later, in imp-core:

- launch subagents only after explicit user/config permission;
- cap concurrent subagents;
- enforce allowed paths;
- aggregate verification evidence;
- parent agent remains responsible for final summary.

Acceptance tests:

- independent path clusters are separated;
- overlapping files are not assigned to concurrent subagents;
- security-sensitive clusters default to main-agent only;
- generated contract includes objective, allowed paths, forbidden actions, verification, and return requirements.

## Suggested v1 cut

The first shipped version should include milestones 1-8.

Defer until after v1:

- actual install execution, if we want more safety review;
- SARIF output;
- after-file-write hooks;
- automatic repair execution;
- subagent launching;
- LLM-backed audit rules.

A good v1 still gives agents a major upgrade over terminal-only linting:

- one compact audit tool;
- repo-native check discovery;
- missing-tool clarity;
- no surprise installs;
- native AI/agent-quality checks;
- external parser normalization;
- baseline-aware summaries.
