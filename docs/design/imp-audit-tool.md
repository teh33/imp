# imp audit tool design notes

Status: research/design sketch from the initial audit-tool survey. This document is not an implementation contract yet.

## Goal

Build an imp-native audit capability that helps agents understand and improve repository quality without dumping raw terminal logs into context.

The tool should:

- discover project-native quality checks;
- run configured checks with safe timeouts and artifact capture;
- normalize external tool output into compact findings;
- add native imp-specific checks where imp has unique context;
- compare findings against a baseline so agents focus on new issues;
- optionally plan bounded repairs and delegate independent clusters to subagents.

The tool should not try to replace mature language/security analyzers. If a domain already has a large mature tool, imp should call and parse it instead of reimplementing it.

## Research snapshot

Representative repositories were cloned outside the imp checkout under `/tmp/imp-audit-research` and counted with `tokei`.

Clone revisions:

| repo | revision | remote |
|---|---|---|
| aislop | `5922154` | `https://github.com/scanaislop/aislop.git` |
| sloppylint | `e0e3ced` | `https://github.com/rsionnach/sloppylint.git` |
| gptlint | `f8bd578` | `https://github.com/gptlint/gptlint.git` |
| scicode-lint | `239cde6` | `https://github.com/authentic-research-partners/scicode-lint.git` |
| gito | `455ba35` | `https://github.com/Nayjest/Gito.git` |
| open-code-review | `9e572b5` | `https://github.com/raye-deng/open-code-review.git` |
| holster-scan | `fb5bde1` | `https://github.com/nauta-ai/holster-scan.git` |
| warden-core | `a2f2ae1` | `https://github.com/alperduzgun/warden-core.git` |

Rough LOC counts:

| repo | license | all LOC | impl-ish code LOC | config/rule-data LOC | impl files | top impl langs |
|---|---|---:|---:|---:|---:|---|
| `aislop` | MIT | 32,007 | 27,960 | 3,490 | 226 | TypeScript 27,448, JavaScript 267, TSX 245 |
| `sloppylint` | unknown | 2,944 | 1,936 | 102 | 17 | Python 1,936 |
| `gptlint` | MIT | 18,460 | 5,330 | 9,167 | 52 | TypeScript 5,319, JavaScript 11 |
| `scicode-lint` | MIT | 53,530 | 18,382 | 7,035 | 496 | Python 18,264, Shell 118 |
| `gito` | MIT | 6,652 | 4,423 | 576 | 59 | Python 4,085, Jinja2 277, RPM Specfile 36, Makefile 25 |
| `open-code-review` | BUSL/BSL | 84,105 | 25,034 | 5,214 | 151 | TypeScript 24,802, JavaScript 196, Shell 36 |
| `holster-scan` | unknown | 1,268 | 1,090 | 83 | 8 | Python 1,087, Autoconf 3 |
| `warden-core` | Apache-2.0 | 194,054 | 116,385 | 2,492 | 625 | Python 114,452, Protocol Buffers 670, Rust 480, Scheme 470 |

`impl-ish` excludes common docs, tests, fixtures, examples, dist, assets, and eval directories. The count is approximate; some rule/config data remains because it is part of the tool's runtime behavior.

Feature-term scan of the research repos:

| repo | json | sarif | baseline | hooks | mcp | diff/changed | fix/repair | score | suppress/ignore | fuzz |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| aislop | 837 | 98 | 225 | 1140 | 42 | 672 | 1529 | 1382 | 300 | 0 |
| sloppylint | 69 | 0 | 0 | 8 | 0 | 3 | 40 | 118 | 69 | 0 |
| gptlint | 265 | 3 | 0 | 29 | 1 | 40 | 116 | 6 | 168 | 4 |
| scicode-lint | 756 | 0 | 33 | 7 | 0 | 315 | 477 | 428 | 80 | 1 |
| gito | 63 | 0 | 0 | 1 | 0 | 189 | 102 | 1 | 11 | 0 |
| open-code-review | 580 | 158 | 14 | 52 | 65 | 488 | 732 | 1499 | 64 | 0 |
| holster-scan | 34 | 25 | 0 | 1 | 7 | 10 | 10 | 1 | 19 | 0 |
| warden-core | 3364 | 524 | 791 | 578 | 1252 | 1516 | 2803 | 1389 | 3099 | 300 |

## Takeaways

1. A 10k production LOC implementation can be a strong v1, but not a flagship agent-quality coordinator.
2. A 20k-25k production LOC ceiling is justified if it includes baseline, adapters, native agent rules, hook/workflow integration, and repair orchestration.
3. The native implementation should not absorb large mature tools. It should call and parse them when installed or configured.
4. The highest-leverage native checks are the ones that only imp can do well because imp sees the agent session, edited files, verification state, tool calls, and workflow context.
5. License boundaries matter. MIT/Apache projects are safer to study conceptually. BUSL/unknown-license projects should be treated as design inspiration only; do not copy code or rule text.

## Crate recommendation

Create a dedicated internal workspace crate:

```text
crates/imp-audit
```

Expose it through a thin imp-core tool wrapper:

```text
crates/imp-core/src/tools/audit.rs
```

Rationale:

- keeps `imp-core/src/tools` from becoming a large audit subsystem;
- allows independent tests, fixtures, and benchmarks;
- makes CLI/TUI/workflow reuse easier;
- keeps audit models usable outside model-facing tool execution;
- gives us a clean boundary for dependencies and LOC growth.

The crate should be imp-native, not a standalone product. `imp-core` should still own agent/session integration, policy-sensitive decisions, and actual subagent launching.

Suggested dependency direction:

```text
imp-audit
  owns pure audit config, discovery, runners, parsers, native rules,
  finding models, baselines, suppressions, summaries, and repair plans.

imp-core
  depends on imp-audit;
  adapts audit reports into ToolOutput;
  provides session/workflow/task context;
  triggers hooks/workflow gates;
  launches subagents from repair plans.
```

## Proposed crate layout

```text
crates/imp-audit/
  Cargo.toml
  src/
    lib.rs
    model.rs
    config.rs
    discover.rs
    scope.rs
    runner.rs
    artifacts.rs
    baseline.rs
    suppressions.rs
    summarize.rs
    repair_plan.rs

    adapters/
      mod.rs
      generic.rs
      sarif.rs
      cargo.rs
      eslint.rs
      ruff.rs
      semgrep.rs
      gitleaks.rs
      osv.rs
      aislop.rs

    native/
      mod.rs
      rules.rs
      text.rs
      rust.rs
      typescript.rs
      python.rs
      agent_safety.rs

    orchestration/
      mod.rs
      cluster.rs
      subagent_contract.rs
```

## Tool shape

Model-facing schema should remain small even if the implementation grows:

```json
{
  "action": "discover | run | explain | baseline | plan_repair | repair",
  "profile": "quick | full | quality | ai_quality | security | deps | secrets | fuzz",
  "scope": "changed | staged | workspace | files",
  "paths": [],
  "base": "HEAD",
  "max_findings": 20,
  "timeout_seconds": 120,
  "repair": {
    "mode": "none | plan | subagents",
    "max_subagents": 3
  }
}
```

Default agent call:

```json
{
  "action": "run",
  "profile": "quick",
  "scope": "changed",
  "max_findings": 20
}
```

The tool should return compact text plus structured details. Full logs belong in artifacts, not chat context.

Example compact output:

```text
Audit failed: 6 new findings, 41 existing findings ignored.

Top blockers:
1. audit/unsafe-hook-command — .imp/audit.toml:42
   Blocking hook runs a shell command after every edit without timeout.
2. clippy::unwrap_used — crates/imp-audit/src/config.rs:91
   Config parse can panic on invalid user TOML.

Repair plan:
- Cluster A: config/parser robustness, 2 findings, safe for subagent
- Cluster B: audit hook safety, 1 finding, main-agent only
- Cluster C: native rule cleanup, 3 findings, safe for subagent

Artifacts:
.imp/audit/runs/2026-06-20T.../clippy.stderr.log
```

## Configuration

Use `.imp/audit.toml` for checks, profiles, thresholds, suppressions, and audit-specific hook bindings.

Example:

```toml
[defaults]
scope = "changed"
base = "HEAD"
timeout_seconds = 120
max_findings = 20
fail_on = ["error"]
baseline = "auto"

[profiles.quick]
description = "Fast checks after agent edits"
checks = ["native-agent-quality", "repo-format", "repo-lint"]

[profiles.full]
description = "Full repo verification"
checks = ["repo-format", "repo-lint", "repo-test"]

[profiles.security]
description = "Secrets, dependencies, and security scanners"
checks = ["secrets", "deps", "semgrep"]

[profiles.ai_quality]
description = "AI-generated code quality"
checks = ["native-agent-quality", "aislop"]

[[checks]]
id = "native-agent-quality"
kind = "ai_quality"
engine = "native"
paths = ["**/*.rs", "**/*.ts", "**/*.py", "**/*.go"]

[[checks]]
id = "repo-format"
kind = "format"
command = "cargo fmt --check"
paths = ["**/*.rs"]
parser = "generic"

[[checks]]
id = "repo-lint"
kind = "lint"
command = "cargo clippy --workspace --all-targets"
paths = ["**/*.rs"]
parser = "cargo"
optional = true

[[checks]]
id = "aislop"
kind = "ai_quality"
command = "aislop scan --changes --json"
parser = "aislop_json"
optional = true
install_hint = "Install with npm, pipx, or brew, or disable this check."

[repair]
enabled = false
default_mode = "plan"
max_subagents = 3
require_clean_worktree = false
verify_profile = "quick"
```

Audit-specific hooks can live in `audit.toml`, but active hooks should be opt-in or conservative:

```toml
[[hooks]]
event = "on_agent_end"
profile = "quick"
scope = "changed"
blocking = true
enabled = true

[[hooks]]
event = "after_file_write"
profile = "quick"
scope = "changed"
blocking = false
debounce_ms = 2000
enabled = false
```

Recommended init behavior:

```text
No audit.toml:
  audit discovery works;
  no automatic background hooks.

imp audit init:
  writes checks/profiles;
  writes hooks disabled or closeout-only.

imp audit init --hooks:
  enables on-agent-end quick audit;
  after-file-write remains opt-in unless explicitly requested.
```

## Native checks to implement

Native checks should focus on high-signal, low-dependency, agent-relevant findings.

Initial families:

- placeholder implementations: `todo!`, `unimplemented!`, `pass`, stub returns, TODO stubs;
- swallowed errors: empty `catch`/`except`, ignored `Result`, broad catch with no action;
- debug leftovers: `dbg!`, `console.log`, `print` in non-CLI contexts;
- unsafe broad casts or suppressions: TypeScript `as any`, unreasoned `type: ignore`;
- narrative/trivial/hedging comments: obvious comments and "hopefully/should work" markers;
- maintainability: oversized functions/files, deep nesting, too many parameters;
- agent-safety: missing verification after edits, unsafe shell/process changes, hooks without timeouts, config expanding file/network/secret exposure;
- dependency hygiene: new dependency declared without configured audit or justification.

Native rules should prefer changed files by default. Workspace-wide findings should be summarized separately to avoid drowning agents in legacy debt.

## External adapters

Prefer adapters over reimplementation for mature domains:

- format/lint/typecheck: cargo fmt, clippy, ESLint, Biome, Ruff, mypy, pyright, gofmt, go vet, golangci-lint;
- tests: cargo test, pytest, jest/vitest, go test;
- security/secrets/deps: Semgrep, Gitleaks, OSV, cargo-audit, npm audit, govulncheck;
- AI quality: aislop, sloppylint, gptlint, scicode-lint, Gito, Warden, Open Code Review when installed/configured and license/use is acceptable;
- universal exchange: SARIF ingest and eventually SARIF output.

Adapters should expose install hints but never install tools by default.

## Tool availability and installation UX

The audit tool should make missing programs visible without turning imp into a package manager that surprises users.

Principles:

- Discovery and normal audit runs may probe for tools on `PATH`, but must not install anything.
- Missing optional checks should not fail the audit by default. They should produce a `skipped` check with an actionable install hint.
- Missing required checks should fail as `blocked`, not as a raw command-not-found error.
- Installation offers must require explicit user confirmation in interactive UI and an explicit flag in headless/CI mode.
- Install commands should be declarative data, not model-generated shell snippets.
- imp should prefer project-local/dev-dependency installation when the ecosystem supports it, and global/user installs only when the user chooses them.
- No install flow should run network/package-manager commands from hooks automatically.

Availability model:

```rust
struct ToolRequirement {
    id: String,
    binary: String,
    version_command: Option<Vec<String>>,
    version_requirement: Option<String>,
    required_for_profiles: Vec<String>,
    optional_for_profiles: Vec<String>,
    installers: Vec<InstallRecipe>,
}

struct InstallRecipe {
    id: String,
    label: String,
    manager: String,
    command: Vec<String>,
    scope: InstallScope,
    platform: Vec<Platform>,
    mutates_project: bool,
    requires_network: bool,
}
```

Example check configuration:

```toml
[[checks]]
id = "aislop"
kind = "ai_quality"
command = "aislop scan --changes --json"
parser = "aislop_json"
optional = true
requires = ["aislop"]

[[requirements]]
id = "aislop"
binary = "aislop"
version_command = ["aislop", "version"]

[[requirements.installers]]
id = "npm-dev"
label = "Add aislop as a project dev dependency"
manager = "npm"
command = ["npm", "install", "--save-dev", "aislop"]
scope = "project"
mutates_project = true
requires_network = true

[[requirements.installers]]
id = "npx-ephemeral"
label = "Run through npx without adding a dependency"
manager = "npx"
command = ["npx", "--yes", "aislop@latest", "scan"]
scope = "ephemeral"
mutates_project = false
requires_network = true
```

Discovery output should include an availability table:

```text
Audit tools
✓ cargo fmt        available  rust format
✓ cargo clippy     available  rust lint
○ gitleaks         missing    optional for secrets
○ aislop           missing    optional for ai_quality
✗ semgrep          missing    required for configured security profile
```

A missing optional tool during `audit run` should be compact:

```text
Skipped optional check `aislop`: command `aislop` was not found.
Install options:
  1. npm install --save-dev aislop        project dev dependency
  2. brew install scanaislop/tap/aislop   user/global install
  3. disable check `aislop` in .imp/audit.toml
```

Interactive offer flow:

```text
The `ai_quality` profile can use 1 missing optional tool:
- aislop: catches AI-generated code quality issues.

Install now?
  [ ] npm project dev dependency: npm install --save-dev aislop
  [ ] Homebrew user install: brew install scanaislop/tap/aislop
  [x] Skip for now
```

Headless/CI behavior:

- `audit run` only reports missing tools and skips/blocks according to config.
- `audit doctor --json` emits machine-readable missing requirements.
- `audit install --check <id> --recipe <id> --yes` performs an explicit install.
- `audit run --offer-installs` may ask in TUI/interactive modes, but should error in non-interactive mode unless `--yes` and an exact recipe are provided.

Install safety:

- Show exact command, working directory, files likely to change, network requirement, and whether the recipe is project-local or global.
- Refuse project-mutating installs when the worktree has overlapping dirty package files unless the user confirms.
- Record install attempts and outputs as audit artifacts.
- Never install from a hook, subagent repair, or model tool call without an explicit user approval path.
- Prefer pinned or project-configured versions over `latest` for repeatability.

The model-facing `audit` tool should not generally install programs itself. If installation is exposed to agents, it should be a separate high-risk action such as `audit_install` or `audit action=install` that routes through normal user confirmation and policy checks.

## Baseline and suppressions

Baseline is mandatory for usefulness in real repos.

The report should distinguish:

```text
new findings
existing findings
fixed findings
changed-file findings
workspace findings
```

Default closeout behavior should focus on new findings in changed files.

Suppression should support at least:

- inline suppressions with rule id and reason;
- path/rule suppressions in config;
- baseline entries generated from a known state;
- explicit bypass reasons for high/critical findings.

## Repair orchestration and subagents

Repair orchestration is the feature that can make this more than a linter wrapper.

Flow:

```text
1. run audit
2. normalize findings
3. baseline-filter findings
4. cluster related findings
5. classify clusters by safety/delegability
6. produce a repair plan
7. optionally spawn bounded subagents
8. verify each cluster
9. parent agent integrates results or reports leftovers
```

Cluster by:

- paths/modules/crates/packages;
- rule kind;
- changed-vs-legacy status;
- dependency graph relation;
- required verification command;
- conflict probability;
- security sensitivity.

Cluster classifications:

```text
auto_fixable
agent_fixable
requires_main_agent
requires_user_decision
ignore_existing
```

Example subagent contract:

```json
{
  "objective": "Fix audit findings in cluster A only.",
  "allowed_paths": [
    "crates/imp-audit/src/native/**"
  ],
  "findings": ["A001", "A002", "A003"],
  "forbidden": [
    "Do not edit public tool schemas.",
    "Do not run network or package-install commands.",
    "Do not touch unrelated findings."
  ],
  "verification": [
    "cargo test -p imp-audit native"
  ],
  "return": [
    "summary",
    "files changed",
    "findings fixed",
    "verification evidence",
    "remaining risks"
  ]
}
```

Default safety constraints:

- repair planning is safe;
- subagent repair is opt-in/configured;
- cap concurrent subagents;
- delegate only non-overlapping path clusters;
- do not delegate security-sensitive or architecture-changing clusters by default;
- each cluster must declare verification;
- parent remains responsible for final integration and reporting.

## LOC targets

Tests excluded:

```text
MVP:        8k-12k production LOC
Good v1:   14k-18k production LOC
Excellent: 20k-25k production LOC
```

The 25k target should buy adapter breadth, baseline/suppression lifecycle, native agent-safety rules, and repair orchestration. It should not mean reimplementing Warden/Semgrep/CodeQL/Sonar.

## Implementation phases

1. Core crate skeleton and models
   - `imp-audit` crate;
   - finding/check/profile/report types;
   - compact summary renderer.

2. Discovery, scope, and runner
   - changed/staged/workspace/files scope;
   - command runner with timeout/cancellation;
   - artifact capture/truncation;
   - generic parser.

3. Config and profiles
   - `.imp/audit.toml` loading;
   - profile/check selection;
   - optional checks and install hints;
   - no surprise package install or network access.

4. Baseline and suppressions
   - finding fingerprinting;
   - new/existing/fixed split;
   - basic inline and config suppressions.

5. Native rules
   - placeholder, swallowed error, debug leftovers, broad casts, comments, size/nesting;
   - imp-specific agent-safety checks.

6. External adapters
   - SARIF;
   - cargo/clippy;
   - Ruff;
   - ESLint/Biome;
   - Gitleaks/Semgrep/OSV;
   - aislop/sloppylint where present.

7. Tool, hooks, and workflow integration
   - model-facing `audit` tool wrapper;
   - closeout verification integration;
   - opt-in audit hooks;
   - TUI/headless summary rendering as needed.

8. Repair planning and subagent orchestration
   - finding clusters;
   - repair plan action;
   - bounded subagent contracts;
   - verification aggregation.

## Open decisions

- Whether the model-facing tool should be named `audit`, `audit_scan`, or `quality`.
- How much session/task context should be passed into `imp-audit` vs kept in `imp-core`.
- Whether audit hooks should live only in `.imp/audit.toml` or also compile into the existing general hook config.
- Whether SARIF output is v1 or v2.
- Which native language rules are high-confidence enough for default `quick` profile.
- How to expose repair orchestration without surprising users or fighting dirty worktrees.
