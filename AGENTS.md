# AGENTS.md — imp

Read `../AGENTS.md` first. This file adds project-specific guidance for work inside the `imp` repo. Keep it focused on imp-specific behavior; general agent performance belongs in the global agent defaults.

## What imp is

`imp` is the agent engine and native worker/runtime in the Tower repo. It runs as a TUI, one-shot CLI, and JSONL RPC worker. It provides structured tools, durable sessions, workflow-backed planning/verification, provider integrations, policy checks, and Lua extension support.

For future-facing architecture work, use this vocabulary:
- `mana` = platform
- `imp` = agent + default human-facing environment on mana
- `runtime` = live execution layer
- `graph` = durable layer
- `extension` = packaged extensibility
- `action` = preferred system behavior term
- `task` = preferred work term

Use this vocabulary in new architecture docs and migration plans, but do not mechanically rewrite older docs or code when current names are more accurate.

## Product priorities

Prioritize work that improves:
- agent quality and turn-loop behavior;
- runtime boundaries and policy enforcement;
- context assembly, compaction, and evidence handling;
- native tool behavior and UX;
- provider/model integration;
- session durability and structured outcomes;
- workflow execution and verification;
- embedding/hostability for apps built on mana;
- extension seams and safe extensibility.

When in doubt, trust the intelligence of the model and the shape of the existing code. Prefer judgment over rigid process, but ground changes in inspected files and verified behavior.

## Repository map

Workspace crates:
- `crates/imp-core`: agent runtime, tools, sessions, policies, workflows, context, and core domain behavior.
- `crates/imp-llm`: provider/model abstractions and LLM API integration.
- `crates/imp-cli`: CLI entrypoints, setup/auth flows, headless prompt/RPC modes, and import/install helpers.
- `crates/imp-tui`: terminal UI, app state, rendering, input/event loop, and runtime signal handling.
- `crates/imp-lua`: Lua extension runtime for tools, slash commands, and hooks.
- `crates/imp-gui`: experimental GUI surface.
- repo root package: source-install shim so `cargo install --path .` works from the repo root.

Useful docs:
- `README.md`
- `imp_ontology.md`
- `imp_rebuild_plan.md`
- `docs/design/imp-core-crate-boundary-plan.md`
- `../docs/architecture/mana-platform-target-architecture.md`

## Architecture guardrails

Use `docs/design/imp-core-crate-boundary-plan.md` as the current crate-boundary planning note. It is a proposal, not a command to split crates preemptively, but new architecture work should preserve its intent unless there is a better inspected reason.

- Keep product-facing behavior cohesive. Native model-facing tools should stay together; extract heavy engines underneath them rather than scattering tool UX across many crates.
- Treat `imp-core` as the domain/API center, not the default home for every implementation detail. New heavyweight dependencies should not be added to `imp-core` without a clear boundary rationale.
- Prefer composition boundaries such as `imp-bin`, `imp-runtime`, `imp-state`, `imp-tools`, and `imp-codeintel` when they make dependencies, hostability, or testing cleaner.
- Distinguish live runtime from durable graph/state: runtime owns turn execution, cancellation, tool dispatch, and event flow; state owns sessions, indexes, memory, traces, and evidence artifacts.
- Keep headless/RPC behavior separable from TUI behavior. Avoid introducing new `imp-cli -> imp-tui` coupling; compose user-facing modes at the binary/application boundary.
- Do not create microcrates just to reduce line count. Split when it removes dependency pressure, clarifies ownership, improves testability, or protects a stable API seam.
- When a change materially affects crate boundaries, public SDK shape, durable formats, extension contracts, or runtime/state ownership, update the relevant architecture docs in the same change.

For crate-boundary work, include a short rationale in the change summary or docs that answers:
- what dependency or ownership pressure is being reduced;
- which public behavior is preserved;
- which checks prove the moved behavior still works;
- whether compatibility re-exports are temporary or intended API.

## Execution standard

Agents working in this repo should build the right version of a change, not the smallest demo that technically satisfies the words of a request.

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

## Ownership boundaries

Put work in `imp/` when it concerns agent behavior, runtime execution, context assembly, tool registration/UX, provider/model integration, session behavior, execution policy, agent-facing interfaces, or embedding surfaces for apps built on mana.

Escalate to root architecture work when a change affects the mana/imp split, runtime vs graph boundaries, extension contracts, cross-app platform APIs, or Tower-wide naming/ontology.

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

For this local machine, if `origin` points at `file://$HOME/git-server/repos/*.git`, agents may create focused commits and push to the local origin after verified work. Still ask before destructive history operations, force pushes, deleting unmerged work, or touching non-local/network remotes.
