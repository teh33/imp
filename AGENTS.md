# AGENTS.md — imp

imp's durable product and engineering compass. Keep snapshots, crate maps, and backlogs in maintained architecture, readiness, or planning artifacts.

## Product

imp is a local-first agent engine and native runtime for serious software work. Its promise:

> Given a difficult repository task with real constraints, imp can understand the codebase, plan and execute the work with minimal waste, survive interruption, verify the result, and show why the work is complete.

Optimize in this order:

1. correct completion of substantial repository work;
2. user control, safety, and clear runtime behavior;
3. speed, context efficiency, and low waste;
4. extensibility that preserves the first three.

## Principles

- Outcomes over activity: judge imp by completed repository work, not turns, tool calls, plans, workers, or feature count.
- User authority: autonomy must increase the user's leverage without obscuring intent, scope, cost, effects, or control.
- Local authority: providers may propose actions; the runtime owns policy, effects, persistence, recovery, and completion. Never assume replay is safe after a possible effect.
- Durable execution: objectives, constraints, decisions, progress, effects, and evidence must survive compaction, interruption, and process death.
- Interruption by design: cancellation, steering, provider failure, and partial effects are normal execution states, not exceptional cleanup.
- Deliberate context: preserve what governs the work, retrieve what informs the next decision, and discard what no longer earns its cost.
- Proof before completion: acceptance criteria and evidence determine completion. Confidence, narration, and code presence do not.
- One semantic core: user-facing and host-facing adapters may differ in presentation, never in policy, state, recovery, or completion semantics.
- Depth before breadth: finish one production-quality path through success and failure before adding variants, integrations, or surface area.
- A small core: admit features only when they strengthen serious repository work; isolate extensions and reject unrelated platform growth.
- Earned trust: tools, hooks, extensions, workers, providers, and external content cannot self-authorize, erase provenance, or weaken host policy.
- Measured efficiency: improve time, attention, tokens, memory, and reliability against a baseline; do not trade correctness or control for motion.

## Development direction

Choose work by maturity:

- Scaffold: prove one narrow production-quality path before adding breadth.
- Partial: close normal, failure, and integration gaps before adding adjacent features.
- Mature: preserve contracts, prevent regressions, measure claims, and simplify.

Improve the weakest link in the end-to-end outcome before adding adjacent surface area. Avoid parallel architectures.

## Readiness

When `.readiness.yaml` covers the affected capability:

1. run `ready find "<task terms>" --limit 10 --json`;
2. inspect candidates with `ready show <selector> --json`;
3. treat `any-term-fallback` results as candidates, not exact matches;
4. record evidence-backed assessments, gaps, and discoveries atomically with `ready add --stdin` or `ready apply --stdin`.

Use `ready next` only for explicit readiness or backlog work. Never infer readiness from code presence or close criteria with material gaps.

## Rust tests

Run test binaries with `cargo nextest run`; use `cargo test` only for doctests or cases nextest cannot run.
