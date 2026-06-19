# imp technical docs

This directory contains technical reference pages for imp. README.md is the entrypoint; these pages cover details that are too specific for the README.

## Core references

- [Workflows](workflows.md) — workflow artifacts, schema, lifecycle, events, prototyping, verification, closeout, and API direction.
- [ACP editor adapter scaffold](acp.md) — internal/out-of-scope for 0.3.0 unless separately verified; current limitations and editor launch shape.
- [RPC protocol](rpc.md) — `--mode rpc`, stdin commands, stdout events, `--runtime-json`, and host integration notes.
- [Scan tool](scan-tool.md) — tree-sitter-backed code discovery, extraction, structural search, related-symbol lookup, file discovery, and verification notes.
- [Native tools](tools.md) — built-in tools, mutability, policy interaction, execution behavior, and display notes.
- [Runtime policy](policy.md) — modes, autonomy, tool allow/deny rules, write-path rules, hooks, and verify gates.
- [Sessions and evidence](sessions.md) — JSONL session records, branches, compaction, traces, evidence packets, and recovery.
- [Lua extensions](extensions-lua.md) — shipped Lua extension runtime, load paths, custom tools, slash commands, hooks, and capabilities.
- [Architecture](architecture.md) — crate responsibilities, runtime flow, provider layer, workflow core, UI/CLI/RPC surfaces, and extension runtime.
