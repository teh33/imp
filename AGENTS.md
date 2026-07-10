# AGENTS.md — imp

Read `../AGENTS.md` first. This file adds project-specific guidance for work inside the `imp` repo. Keep it focused on imp-specific behavior; general agent performance belongs in the global agent defaults.

## What imp is

`imp` is the agent engine and native worker/runtime. It runs as a TUI, one-shot CLI, and JSONL RPC worker. It provides structured tools, durable sessions, workflow-backed planning/verification, provider integrations, policy checks, and Lua extension support.

## Product priorities

Prioritize work that improves:
- professional engineering quality
- agent quality and turn-loop behavior;
- runtime boundaries and policy enforcement;
- context assembly, compaction, and evidence handling;
- native tool behavior and UX;
- provider/model integration;
- session durability and structured outcomes;
- workflow execution and verification;
- embedding/hostability for apps
- extension seams and safe extensibility.

When in doubt, trust the intelligence of the model and the shape of the existing code. Prefer judgment over rigid process, but ground changes in inspected files and verified behavior.

## Repository map

Workspace crates:
- `crates/imp-core`: agent runtime, tools, sessions, policies, workflows, context, and core domain behavior.
- `crates/imp-llm`: provider/model abstractions and LLM API integration.
- `crates/imp-cli`: CLI entrypoints, setup/auth flows, headless prompt/RPC modes, and import/install helpers.
- `crates/imp-tui`: terminal UI, app state, rendering, input/event loop, and runtime signal handling.
- `crates/imp-lua`: Lua extension runtime for tools, slash commands, and hooks.
- `crates/imp-gui`: experimental GUI surface, postponed.
- repo root package: source-install shim so `cargo install --path .` works from the repo root.

Useful docs:
- `README.md`

## Execution standard

Agents working in this repo should build the correct version of a change, not the smallest demo that technically satisfies the words of a request.

- Start from intended product/runtime behavior and make the implementation match that intent.
- Prefer complete, durable solutions over thin shims, placeholders, mocked behavior, or happy-path-only partial paths.
- Keep changes focused, but do not underbuild core behavior just to minimize the diff.
- Continue through implementation, tests, and verification until complete, blocked by evidence, or needing a user decision.
- Use small reversible edits as an execution technique, not as a reason to deliver partial behavior.
- Follow the real control flow, persistence model, policy boundaries, error handling, and user-facing UX.
- Preserve existing architecture when it is sound; improve the seam when the current seam would force a brittle or fake solution.
- Include meaningful tests or verification for behavior that matters, including important failure and edge paths.
- Do not leave TODO-driven behavior, silent fallbacks, or intentionally incomplete implementations unless explicitly agreed and documented.
- If the complete solution is materially larger than expected, pause and explain the scope tradeoff instead of silently shipping a minimal substitute.

## Working in this Rust workspace

- Prefer `scan` for symbol-aware lookup before broad text search when code structure matters.
- Match existing Rust style and error-handling patterns before introducing new abstractions.
- Keep public API churn minimal unless the API is the problem.
- Prefer explicit state and typed domain models over stringly typed runtime behavior.
- Treat policy, tool execution, secret handling, provider traffic, and file mutation paths as security-sensitive.
- Preserve durable session/workflow formats unless the change explicitly includes migration or compatibility handling.
- Do not describe future TypeScript extension support as shipped. Current shipped extension support is Lua.

## Readiness inventory

- `.readiness.yaml` is imp's exploratory product-readiness ledger. When work affects an inventoried capability, discover relevant criteria with `ready find "<task terms>" --limit 10 --json`, then inspect selected criteria with `ready show <selector> --json`. Treat `any-term-fallback` results as candidates, not exact matches. Use `ready next` only when the user asks for readiness/backlog work.
- After evidence-backed changes, update related assessments, gaps, and discoveries through atomic `ready add --stdin` or `ready apply --stdin` batches. Do not raise levels from code presence alone, mark criteria complete with unresolved gaps, or treat the current inventory as exhaustive.

## Verification

Use the narrowest meaningful check for the files touched.

Common checks:
- `cargo fmt --check`
- `cargo check -p <crate>`
- `cargo test -p <crate> <test_name>` for targeted tests
- `cargo test -p <crate>` for crate-level behavior
- `cargo check --workspace` when touching shared types, workspace dependencies, or cross-crate APIs

For docs-only changes, inspect the rendered/changed Markdown enough to catch broken structure and run lightweight checks such as `git diff --check` when the file is tracked.

If a check fails, fix the root cause or report the blocker with the exact command and error. Do not claim unverified success.

## Module organization

Prefer local `AGENTS.md` files over crate READMEs for future agent instructions.

When decomposing large files, preserve behavior first:
- split by responsibility, not line count;
- keep public API churn minimal unless the API is the problem;
- move tests with the behavior they protect when practical;
- avoid mixing mechanical moves with semantic changes;
- run the narrowest crate-level check after each extraction.

Current high-priority decomposition targets:
- `crates/imp-tui/src/app.rs`: app state, event loop, runtime signals, auth/secrets flow, render caches, agent event handling.
- `crates/imp-core/src/agent.rs`: turn loop, tool execution, next-action assessment, retry handling, mode/policy enforcement.
- `crates/imp-cli/src/lib.rs`: args, auth/setup, headless worker mode, RPC mode, chat shell, import/install helpers.
- `crates/imp-core/src/tools/workflow.rs`: workflow schema/action dispatch, native run orchestration, run-state persistence, rendering, policy.

## Git hygiene

This repo may have unrelated dirty files. Inspect status before edits and do not overwrite user changes outside the requested files.

When finished with work, ask to commit. Still ask before destructive history operations, force pushes, deleting unmerged work, or touching non-local/network remotes.
