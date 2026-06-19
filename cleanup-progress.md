# imp cleanup progress

Tracking cleanup against `goals.md` on branch `cleanup/remove-gui-mcp-workflows`.

## Metrics

Baseline is commit `1ddfefc9c` before this cleanup branch, excluding `Cargo.lock` from LOC percentage because lockfile churn is generated dependency metadata.

| Metric | Value |
| --- | ---: |
| Baseline tracked source/docs/config LOC | 157,754 |
| Current tracked source/docs/config LOC | 143,563 |
| Source/docs/config insertions | 6,145 |
| Source/docs/config deletions | 20,336 |
| Source/docs/config net LOC | -14,191 |
| Cleanup by net LOC reduction | 9.00% |
| Cleanup by removed LOC | 12.89% |
| Full diff including `Cargo.lock` | +6,221 / -22,501, net -16,280 |

## Goal status

| Goal | Status | Notes |
| --- | --- | --- |
| Remove experimental GUI crate and workspace references | Done | Removed `crates/imp-gui`, workspace membership, GUI deps, and current docs references. |
| Remove TypeScript extension support/docs | Done | Removed `typescript_extensions` implementation and bridge docs. Kept `tree-sitter-typescript` for source scanning. |
| Remove MCP support/config/docs/deps | Done | Removed MCP shim and `imp mcp`; ACP still rejects incoming `mcpServers` for protocol compatibility. |
| Remove obsolete learning/memory/soul code | Done | Removed core modules, memory tool, TUI `/memory`, config, storage migration, and prompt injection. |
| Decompose godfiles | In progress | Workflow tool postponed; CLI/TUI/core decomposition remains future cleanup. |
| Workflow tool reimagining | Postponed | User explicitly postponed this slice. |
| Wiki-style docs cleanup | Done | Removed archival design/rebuild/proposal/plan docs from the wiki-style docs set. |

## Verification

Latest verified commands:

- `cargo fmt --check`
- `cargo test -p imp-tui slash`
- `cargo check --workspace`
