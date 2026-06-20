# imp cleanup progress

Tracking cleanup against `goals.md` on branch `cleanup/remove-gui-mcp-workflows`.

## Metrics

Baseline is commit `1ddfefc9c` before this cleanup branch, excluding `Cargo.lock` from LOC percentage because lockfile churn is generated dependency metadata.

| Metric | Value |
| --- | ---: |
| Baseline tracked source/docs/config LOC | 157,754 |
| Current tracked source/docs/config LOC | 143,714 |
| Source/docs/config insertions | 11,268 |
| Source/docs/config deletions | 25,643 |
| Source/docs/config net LOC | -14,375 |
| Cleanup by net LOC reduction | 9.09% |
| Cleanup by removed LOC | 16.22% |
| Full diff including `Cargo.lock` | 139 files changed, 11344 insertions(+), 27808 deletions(-) |
| Non-test godfiles remaining (`>=1000` LOC) | 9 |
| Test godfiles remaining (`>=1000` LOC) | 6 |

## Goal status

| Goal | Status | Notes |
| --- | --- | --- |
| Remove experimental GUI crate and workspace references | Done | Removed `crates/imp-gui`, workspace membership, GUI deps, and current docs references. |
| Remove TypeScript extension support/docs | Done | Removed `typescript_extensions` implementation and bridge docs. Kept `tree-sitter-typescript` for source scanning. |
| Remove MCP support/config/docs/deps | Done | Removed MCP shim and `imp mcp`; ACP still rejects incoming `mcpServers` for protocol compatibility. |
| Remove obsolete learning/memory/soul code | Done | Removed core modules, memory tool, TUI `/memory`, config, storage migration, and prompt injection. |
| Decompose godfiles | In progress | Workflow tool postponed; CLI/TUI/core decomposition continues. Extracted agent context recovery/run artifact helpers and git worktree actions; 9 non-test godfiles remain. |
| Workflow tool reimagining | Postponed | User explicitly postponed this slice. |
| Wiki-style docs cleanup | Done | Removed archival design/rebuild/proposal/plan docs from the wiki-style docs set. |

## Verification

Latest verified commands:

- `cargo fmt --check`
- `cargo check -p imp-core`
- `git diff --check`
