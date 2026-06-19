# AGENTS.md — imp

This file adds only imp-specific guidance. Keep general behavior in `~/.imp/agents.md` and machine-local Git rules in home-directory agent instructions.

- Product: `imp` is an agent engine and native worker/runtime for TUI, one-shot CLI, and JSONL RPC use.
- Scope: structured tools, durable sessions, workflows, provider integrations, policy checks, and Lua extensions.
- Priorities: agent/runtime quality, policy boundaries, context/evidence handling, native tool UX, provider integration, session durability, workflow verification, hostability, and safe extensibility.

## Workspace

- `crates/imp-core`: agent runtime, tools, sessions, policies, workflows, context.
- `crates/imp-llm`: provider/model abstractions and LLM APIs.
- `crates/imp-cli`: CLI, auth/setup, headless/RPC, chat shell, import/install helpers.
- `crates/imp-tui`: terminal UI, app state, rendering, input/event loop, runtime signals.
- `crates/imp-lua`: Lua tools, slash commands, and hooks.
- root package: `cargo install --path .` shim.

## Standards

- Build durable behavior, not demo shims; follow real control flow, persistence, policy, errors, and UX.
- Prefer typed domain models, explicit state, and narrow APIs over stringly or speculative abstractions.
- Treat policy, tool execution, secrets, provider traffic, and file mutation as security-sensitive.
- Preserve public behavior and durable session/workflow formats unless compatibility is handled.
- Shipped extension support is Lua; do not present TypeScript support as shipped.

## Verification

Use the narrowest meaningful check:

- `cargo fmt --check`
- `cargo check -p <crate>`
- `cargo test -p <crate> <test_name>`
- `cargo test -p <crate>`
- `cargo check --workspace` for shared or cross-crate changes

For docs-only changes, inspect Markdown and run `git diff --check` when tracked.

## Module organization

Prefer local `AGENTS.md` files over crate READMEs. Split large files by responsibility; preserve behavior; minimize public API churn; move relevant tests; avoid mixing mechanical moves with semantic changes.

High-priority decomposition targets: `crates/imp-tui/src/app.rs`, `crates/imp-core/src/agent.rs`, `crates/imp-cli/src/lib.rs`, `crates/imp-core/src/tools/workflow.rs`.

## Git hygiene

Unrelated dirty files may exist. Inspect status, avoid unrelated work, stage only intentional paths, and ask before commits or destructive history operations.
