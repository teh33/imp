# imp-audit goals

Status: working goals for the `imp-audit` crate and its imp integration.

## North star

`imp-audit` should give imp agents a compact, repo-aware quality signal that is more useful than raw terminal lint output.

It should help an agent answer:

- What checks matter for this repo and this change?
- Which tools are available or missing?
- Which findings are new versus existing debt?
- Which findings block safe closeout?
- Which issues can be safely delegated or repaired later?

## Product goals

1. **Agent-focused output**
   - Return concise summaries, top blockers, counts, and artifact references.
   - Avoid flooding model context with raw linter logs.
   - Preserve full logs as artifacts for inspection.

2. **Repo-native first**
   - Discover and run the repo's own checks when possible.
   - Prefer configured commands over imp guesses.
   - Respect project conventions and existing quality gates.

3. **Call mature tools, do not absorb them**
   - Integrate with large external tools through adapters.
   - Do not reimplement Semgrep, CodeQL, Sonar, Warden, full ESLint/Ruff/Clippy, or full dependency scanners.
   - Native imp checks should focus on agent-specific quality/safety signals.

4. **Safe missing-tool UX**
   - Clearly report missing optional and required tools.
   - Skip optional missing tools by default.
   - Block required missing tools with actionable install hints.
   - Never install tools during normal audit runs.
   - Offer installs only through explicit user-approved flows.

5. **Baseline-aware findings**
   - Separate new, existing, fixed, changed-file, and workspace findings.
   - Default summaries should focus on new findings relevant to the task.
   - Legacy debt should be visible but should not drown the agent.

6. **Configurable without lock-in**
   - Use `.imp/audit.toml` for checks, profiles, requirements, install recipes, suppressions, and optional hooks.
   - Support custom commands and parsers.
   - Keep defaults useful but conservative.

7. **No surprise mutation**
   - Audit runs are read-only except for audit artifacts.
   - Package-manager installs, fixes, and subagent repairs require explicit opt-in.
   - Hooks must not install tools or run expensive/networked work by surprise.

8. **Repair orchestration later**
   - First support planning: group findings into safe repair clusters.
   - Later, allow explicit bounded subagent repair for non-overlapping, low-risk clusters.
   - Parent agent remains responsible for final integration and reporting.

## Non-goals

- Replace every linter, typechecker, security scanner, or test runner.
- Make LLM-backed review part of the default quick path.
- Auto-fix findings in v1.
- Auto-install missing programs from model-facing tool calls.
- Add broad language-specific static analysis beyond high-signal native rules.
- Make hooks noisy or expensive by default.

## v1 success criteria

A useful v1 should include:

- `crates/imp-audit` core models and config loading.
- Profiles, checks, requirements, and install recipes.
- Requirement availability/doctor logic.
- Changed/staged/workspace/files scope support.
- Bounded command runner with timeout and artifact capture.
- Generic parser and at least a small set of external adapters.
- Native high-signal agent-quality rules.
- Compact report summarization.
- Baseline classification for new vs existing findings.
- A thin `imp-core` tool wrapper that exposes discovery/run/doctor behavior.

## Good v1 defaults

Default behavior should be conservative:

```text
audit run profile=quick scope=changed
```

The quick profile should prefer:

- native agent-quality checks on changed files;
- cheap repo-native formatting/lint checks when confidently discovered;
- skipped optional external checks with install hints;
- no network;
- no installs;
- no auto-fixes;
- no subagent execution.

## Quality bar

The implementation should be:

- typed and serializable;
- deterministic by default;
- testable without network access;
- careful with dirty worktrees;
- artifact-oriented for large output;
- clear about skipped, blocked, failed, and passed states;
- small enough to grow deliberately instead of becoming a second product.

## Milestone direction

1. Core models, config, summaries.
2. Requirement availability and doctor output.
3. Scope selection and command runner.
4. Parsers/adapters.
5. Baseline and suppressions.
6. Native agent-quality rules.
7. imp tool integration.
8. Hooks/workflow integration.
9. Repair planning.
10. Explicit subagent orchestration.
