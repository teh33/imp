# imp cleanup progress

Tracking cleanup against `goals.md` on branch `cleanup/remove-gui-mcp-workflows`.

## Metrics

Baseline is the tracked repository before this cleanup branch's current diff, excluding `Cargo.lock` from LOC percentage because lockfile churn is generated dependency metadata.

| Metric | Value |
| --- | ---: |
| Baseline tracked source/docs/config LOC | 157,754 |
| Current tracked source/docs/config LOC | 152,119 |
| Source/docs/config insertions | 57 |
| Source/docs/config deletions | 5,692 |
| Source/docs/config net LOC | -5,635 |
| Cleanup by net LOC reduction | 3.57% |
| Cleanup by removed LOC | 3.61% |
| Full diff including `Cargo.lock` | +133 / -7,857, net -7,724 |

## Goal status

| Goal | Status | Notes |
| --- | --- | --- |
| Remove experimental GUI crate and workspace references | Done | Removed `crates/imp-gui`, workspace membership, GUI deps, and current docs references. |
| Remove TypeScript extension support/docs | Done | Removed `typescript_extensions` implementation and bridge docs. Kept `tree-sitter-typescript` for source scanning. |
| Remove MCP support/config/docs/deps | Done | Removed MCP shim and `imp mcp`; ACP still rejects incoming `mcpServers` for protocol compatibility. |
| Remove obsolete learning/memory/soul code | Done | Removed core modules, memory tool, TUI `/memory`, config, storage migration, and prompt injection. |
| Decompose godfiles | In progress | Workflow tool postponed; CLI/TUI/core decomposition remains future cleanup. |
| Workflow tool reimagining | Postponed | User explicitly postponed this slice. |
| Wiki-style docs cleanup | In progress | Current docs cleaned for removed surfaces; historical design/proposal docs still need pruning or conversion. |

## Verification

Latest verified commands:

- `cargo fmt --check`
- `cargo test -p imp-tui slash`
- `cargo check --workspace`
