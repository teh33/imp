# imp cleanup progress

Tracking cleanup against `goals.md` on branch `cleanup/remove-gui-mcp-workflows`.

## Metrics

Baseline is commit `1ddfefc9c` before this cleanup branch, excluding `Cargo.lock` from LOC percentage because lockfile churn is generated dependency metadata.

| Metric | Value |
| --- | ---: |
| Baseline tracked source/docs/config LOC | 157,754 |
| Current tracked source/docs/config LOC | 144,082 |
| Source/docs/config insertions | 11,635 |
| Source/docs/config deletions | 25,797 |
| Source/docs/config net LOC | -14,007 |
| Cleanup by net LOC reduction | 8.86% |
| Cleanup by removed LOC | 16.32% |
| Full diff including `Cargo.lock` | 141 files changed, 11711 insertions(+), 27962 deletions(-) |
| Non-test godfiles remaining (`>=1000` LOC) | 8 |
| Test godfiles remaining (`>=1000` LOC) | 6 |

## Goal status

| Goal | Status | Notes |
| --- | --- | --- |
| Remove experimental GUI crate and workspace references | Done | Removed `crates/imp-gui`, workspace membership, GUI deps, and current docs references. |
| Remove TypeScript extension support/docs | Done | Removed `typescript_extensions` implementation and bridge docs. Kept `tree-sitter-typescript` for source scanning. |
| Remove MCP support/config/docs/deps | Done | Removed MCP shim and `imp mcp`; ACP still rejects incoming `mcpServers` for protocol compatibility. |
| Remove obsolete learning/memory/soul code | Done | Removed core modules, memory tool, TUI `/memory`, config, storage migration, and prompt injection. |
| Decompose godfiles | In progress | Workflow tool postponed; CLI/TUI/core decomposition continues. Extracted agent context recovery/run artifact helpers and git worktree actions; 8 non-test godfiles remain. |
| Workflow tool reimagining | Postponed | User explicitly postponed this slice. |
| Wiki-style docs cleanup | Done | Removed archival design/rebuild/proposal/plan docs from the wiki-style docs set. |

## Verification

Latest verified commands:

- `cargo fmt --check`
- `cargo check -p imp-core`
- `git diff --check`
