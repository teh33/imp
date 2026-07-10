# AGENTS.md — imp

This file is imp's product and engineering compass. It should remain useful as the implementation changes. Keep transient status, crate inventories, and decomposition backlogs in maintained architecture, readiness, or planning artifacts instead.

General behavior comes from `~/.imp/agents.md`; machine-local Git policy comes from the home-directory instructions. This file adds only imp-specific direction.

## Product direction

imp is a local-first agent engine and native runtime for serious software work. Its core promise is:

> Given a difficult repository task with real constraints, imp can understand the codebase, plan and execute the work with minimal waste, survive interruption, verify the result, and show why the work is complete.

Optimize decisions in this order:

1. correct completion of substantial repository work;
2. user control, safety, and clear runtime behavior;
3. speed, context efficiency, and low operational waste;
4. extensibility that preserves the first three properties.

Prefer depth on the core path over feature count or ecosystem parity. A feature belongs in core when it materially improves the promise above. Otherwise prefer an extension, a separate tool, or an explicit exclusion.

## Enduring boundaries

- Keep policy, tool effects, persistence, recovery, and completion decisions in the local runtime. Providers may propose actions; they do not own those boundaries.
- Keep user-facing and host-facing adapters consistent through shared runtime semantics rather than surface-specific behavior forks.
- Make durable state typed, inspectable, versionable, and recoverable. This includes sessions, workflows, traces, evidence, policy decisions, and worker state.
- Make powerful behavior controllable before making it automatic. Approval, cancellation, steering, scope, and recovery must have explicit semantics.
- Treat tools, hooks, extensions, workers, and external content as constrained inputs. They cannot self-authorize, bypass provenance, or weaken host policy.
- Label experimental and partial surfaces honestly. Code presence is not proof that a capability is shipped, supported, or production-ready.
- Do not expand imp into a broad deployment, team, or enterprise control plane without evidence that doing so strengthens its core product promise.

## Establish current reality

Do not rely on this file for a snapshot of the repository. Before changing behavior:

1. read the applicable instructions and user objective;
2. inspect `Cargo.toml` for current workspace membership and dependency boundaries;
3. inspect `docs/architecture.md` for intended ownership and runtime flow;
4. follow the actual call path, persistence path, policy path, tests, and user-facing adapter involved;
5. check Git state and preserve unrelated work.

Treat implementation and exercised tests as evidence of current behavior, not of product readiness. If code, tests, and documentation disagree, determine the real behavior and fix in-scope drift rather than choosing the most convenient source.

## Choosing direction at any maturity

The user objective defines the required outcome. Use this product compass to resolve ambiguity, choose implementation depth, and evaluate tradeoffs without replacing explicit user intent.

When selecting work or choosing among viable approaches:

1. establish the affected capability's maturity from production wiring, tests, evidence, and known gaps;
2. identify the weakest link in the end-to-end user outcome, not merely the easiest local change;
3. choose the smallest coherent change that removes that bottleneck without creating a parallel architecture;
4. match proof to maturity and risk;
5. leave the capability easier to understand, operate, and verify than before.

For a scaffold, prove one narrow production-quality vertical path before adding breadth. For a partial capability, close normal-path, failure-path, and integration gaps before adding adjacent features. For a mature capability, preserve contracts, prevent regressions, measure important claims, and simplify where possible. At every stage, prefer an evidenced improvement to the core promise over visible but disconnected surface area.

## Engineering standard

- Build the complete durable behavior, not a demo shim or happy-path patch.
- Fix behavior in its owning layer. Avoid adapter-only patches that leave runtime semantics inconsistent.
- Prefer typed domain models, explicit state transitions, narrow APIs, and isolated side effects.
- Reject silent fallbacks, placeholder success, stringly internal protocols, and speculative abstractions.
- Preserve public contracts and durable formats unless the change includes deliberate compatibility or migration handling.
- Treat provider traffic, secrets, policy, tool execution, file mutation, hooks, and extension capabilities as security-sensitive.
- Design cancellation, retries, and recovery around whether side effects may already have occurred. Never assume replay is safe.
- Keep modules cohesive and split them by responsibility when they become difficult to reason about. Do not maintain static decomposition target lists here.
- Prefer removing accidental complexity over adding another layer. Keep non-core or experimental behavior isolated from default paths.
- Account for latency, token use, startup cost, memory, and repeated work on hot paths. Measure material performance claims.

## Agent-quality standard

Changes to the agent or runtime should preserve these properties:

- the objective, constraints, acceptance criteria, and steering survive long runs and compaction;
- each turn advances the task, resolves material uncertainty, performs necessary work, or verifies an outcome;
- context is selected for relevance and provenance instead of accumulated without discipline;
- tool plans and effects remain policy-checked, observable, and attributable;
- interruption and cancellation produce deterministic, reviewable recovery behavior;
- completion follows acceptance criteria and verification, not the last successful tool call;
- final outcomes distinguish success, concerns, blockers, and missing context without hiding failed checks;
- users can understand what happened without reading internal implementation files.

## Shipped and supported claims

Discover current support from production wiring, default registration paths, tests, release configuration, and maintained user documentation. Do not infer it from dormant modules, compatibility code, examples, or plans.

A surface may be described as shipped or supported only when its normal path is wired end to end, its important failure modes are tested, and its user contract is documented. Otherwise label its actual maturity and preserve isolation from stable paths.

## Readiness inventory

When `.readiness.yaml` is present and work affects an inventoried capability:

1. run `ready find "<task terms>" --limit 10 --json`;
2. inspect relevant candidates with `ready show <selector> --json`;
3. treat `any-term-fallback` matches as candidates, not exact results;
4. after evidence-backed changes, update related assessments, gaps, and discoveries atomically with `ready add --stdin` or `ready apply --stdin`.

Use `ready next` only for explicit readiness or backlog work. Never raise readiness from code presence alone or mark a criterion complete while material gaps remain.

## Verification and closeout

Use the narrowest check that proves the changed behavior, then expand when shared contracts or risk require it:

- `cargo fmt --check`
- `cargo test -p <crate> <test_name>`
- `cargo test -p <crate>`
- `cargo check -p <crate>`
- `cargo check --workspace` for shared or cross-crate changes

Test important failure, policy, cancellation, persistence, and recovery paths when affected. For documentation-only changes, inspect the rendered structure and run `git diff --check` when tracked.

Before declaring completion, reconcile the user objective, changed files, tests, durable artifacts, compatibility, and unresolved concerns. Report the result, verification evidence, and material limitations concisely. Never claim unverified success.
