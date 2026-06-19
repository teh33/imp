# imp cleanup goals

## Purpose

Create a focused cleanup branch for removing postponed or unused surfaces, reducing maintenance burden, and reshaping large modules into durable, understandable components.

## Primary removals

- Remove the experimental GUI crate and all workspace references to it.
- Remove TypeScript extension support and documentation that presents it as shipped.
- Remove MCP support and related configuration, docs, tests, and dependencies.
- Remove obsolete learning, memory, and soul code:
  - `crates/imp-core/src/learning.rs`
  - `crates/imp-core/src/memory.rs`
  - `crates/imp-core/src/soul.rs`
  - `crates/imp-core/src/tools/memory.rs`

## Refactoring goals

- Finish decomposing existing godfiles into cohesive modules with narrow responsibilities.
- Keep public behavior stable unless a change is part of an explicit removal.
- Prefer typed domain models and explicit state over stringly or speculative abstractions.
- Remove dead code rather than preserving compatibility shims for unshipped features.
- Keep changes incremental enough to verify and review.

## Workflow tool reimagining

- Reassess `crates/imp-core/src/tools/workflow.rs` from first principles.
- Preserve durable workflow/session data only where compatibility is intentionally supported.
- Make workflow operations typed, inspectable, and easy for agents to reason about.
- Separate workflow storage, validation, execution planning, and tool presentation.
- Improve error messages so workflow failures are actionable.

## Verification expectations

Use the narrowest meaningful checks as work progresses:

- `cargo fmt --check`
- `cargo check -p <crate>`
- `cargo test -p <crate> <test_name>`
- `cargo test -p <crate>`
- `cargo check --workspace` for cross-crate removals or shared API changes

## Working rules

- Keep the cleanup work in `/Users/asher/imp-cleanup` on `cleanup/remove-gui-mcp-workflows`.
- Protect unrelated dirty files in `/Users/asher/imp`.
- Stage only intentional paths.
- Do not push to GitHub without approval.
