# imp documentation

These pages document imp's current shipped or intentionally exposed behavior. Start with the [project README](https://github.com/kfcafe/imp#readme) for installation, quickstart, providers, and CLI examples.

## Runtime

- [Architecture](architecture.md) — crate boundaries and runtime flow.
- [Runtime policy](policy.md) — tool, write, autonomy, hook, and verification policy.
- [Autonomy modes](autonomy-modes.md) — approval levels, hard rails, and worktree isolation.
- [Sessions and evidence](sessions.md) — durable sessions, compaction, recovery, traces, and evidence artifacts.
- [Runtime event and state API](runtime-event-state-api.md) — shared typed events and frontend-neutral snapshots.
- [Trust labels and provenance](trust-labels-and-provenance.md) — authority boundaries for workspace, tool, and external context.

## Tools and orchestration

- [Native tools](tools.md) — built-in model-facing tools and policy behavior.
- [Scan tool](scan-tool.md) — structural code search, extraction, related symbols, and tests.
- [Workflows](workflows.md) — durable workflow artifacts, validation, execution, and closeout.
- [Worktree-auto](worktree-auto.md) — isolated autonomous development and closeout behavior.
- [Role registry](role-registry.md) — built-in role profiles and configuration.

## Interfaces and extensions

- [RPC protocol](rpc.md) — JSONL host protocol.
- [ACP editor adapter](acp.md) — current scaffold status and smoke test.
- [Lua extensions](extensions-lua.md) — shipped extension runtime, manifests, commands, tools, hooks, and capabilities.

## Evaluation

- [Eval candidates](eval-candidates.md) — failure/correction artifacts produced during run closeout.
- [Coding eval runner](eval-runner.md) — pinned, verifier-backed coding evaluations and result comparison.

## Scope

The docs site intentionally excludes superseded implementation plans, one-off audits, release scratchpads, and completed migration proposals. Git history remains the archive for those records. Experimental TypeScript/Pi extension compatibility is not a shipped extension surface; Lua is the supported extension runtime.
