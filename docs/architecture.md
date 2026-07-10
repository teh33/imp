# Architecture

imp is a Rust workspace organized around a local agent runtime. Provider traffic, tool execution, policy checks, sessions, workflows, terminal presentation, command-line protocols, and extensions have explicit owners.

## Crates

| Crate | Responsibility |
|---|---|
| `imp-bin` | Installed `imp` binary and top-level composition. |
| `imp-cli` | Command parsing, setup/auth flows, one-shot and JSONL modes, ACP, evaluations, and helper commands. |
| `imp-core` | Agent loop, native tools, sessions, workflows, policy, recovery, evidence, context, and runtime models. |
| `imp-llm` | Provider/model abstraction, streaming, auth helpers, model metadata, and pricing. |
| `imp-lua` | Shipped Lua extension runtime for tools, slash commands, hooks, and capability policy. |
| `imp-tui` | Terminal UI state, rendering, input, and runtime-signal handling. |
| `imp-gui` | Experimental GUI consumer of shared runtime state; not a default workspace member. |
| `mcp-shim` | Internal protocol shim crate. MCP server management in the public CLI is still a placeholder. |

The repository root package is a source-install shim so `cargo install --path .` works from the workspace root.

## Runtime flow

A normal run follows this shape:

1. `imp-bin` and `imp-cli` resolve TUI, one-shot, JSONL, or ACP mode.
2. Configuration, credentials, provider/model metadata, role, autonomy, and run policy are resolved.
3. `AgentBuilder` constructs an `Agent`, registers native tools, attaches Lua loading, and applies role/tool filters.
4. The runtime assembles model context from conversation state, selected files, project instructions, and workflow/session hints.
5. `imp-llm` streams provider events and tool calls.
6. `imp-core` checks policy and executes tools, then appends observations to context.
7. The loop evaluates task progress, workflow obligations, verification gates, cancellation, and closeout.
8. Session, trace, workflow-contract, evidence, and optional worktree artifacts are persisted locally.
9. CLI, TUI, RPC, or ACP adapters present the result.

Providers do not own tool effects, policy, persistence, or completion decisions. Those remain local runtime responsibilities.

## Tools and policy

Native tools live under `crates/imp-core/src/tools/`. `AgentBuilder::register_native_tools` is the canonical default registration point. Tools expose typed schemas, mutability, policy metadata, and structured output.

Policy checks combine:

- agent mode and role restrictions;
- per-run tool and write policy;
- autonomy mode;
- resource scope and provenance;
- hard rails for dangerous actions;
- hooks and verification obligations.

A tool being registered does not guarantee that a particular invocation is allowed.

## Sessions, evidence, and recovery

Sessions are durable JSONL records managed under `crates/imp-core/src/session/`. Recovery checkpoints record provider and tool-loop boundaries so interrupted runs can distinguish safe retry from side effects requiring review.

Each normal run may create project-local artifacts under `.imp/runs/<run-id>/`, including:

- `trace.jsonl`;
- `workflow-contract.json`;
- `evidence.md`;
- verification and policy logs when produced;
- worktree metadata and diffs for wired worktree runs;
- closeout eval candidates for selected failure outcomes.

The older `run_evidence` HTML/index module still exists as a compatibility surface, but the active agent loop writes the project-local artifacts above.

## Workflows and bounded workers

Workflow artifacts live under `.imp/workflows/<id>/` and are parsed by `crates/imp-core/src/workflow/`. The model-facing implementation is `crates/imp-core/src/tools/workflow/`.

The workflow tool can inspect and validate artifacts, run pending command checks, return a main-agent action contract, or return bounded subagent contracts. Agent-completed work uses `complete_step`; explicit status repair and blockers use `update`.

The `subagent` tool is a policy-bounded adapter over the separate `loopr` executable. It accepts only workflow-generated launch contracts and exposes launch, status, wait, send, and cancel against persisted loopr child identities. Imp owns contract validation, parent policy, and an imp-owned mapping under `.imp/runs/<parent-run-id>/subagents/`; loopr owns the child process, persistent session, and thread lifecycle under `.loopr/`. Durable workflow state remains file-backed and separate from child-run state.

## User-facing surfaces

- **TUI:** `imp-tui` consumes agent/runtime signals and owns terminal-specific interaction state.
- **One-shot/JSONL:** `imp-cli` runs prompts and emits human or structured output.
- **RPC:** `imp --mode rpc` accepts prompt, steer, follow-up, and cancel commands over JSONL.
- **ACP:** `imp acp` implements session creation/load/resume and scaffold prompt handling, but does not yet run live model turns.
- **GUI:** `imp-gui` consumes `imp_core::runtime` models experimentally and is not presented as a shipped primary interface.

## Extensions

`imp-lua` is the supported extension runtime. Lua extensions can register tools, slash commands, hooks, and UI requests through host-owned APIs and capability policy.

TypeScript/Pi extension code remains compatibility/experimental code and is not loaded by the normal builder path. It should not be presented as a shipped extension system.

## Planned or partial surfaces

These surfaces must remain labeled partial until their production wiring is verified:

- live ACP agent turns and permission bridging;
- MCP server management;
- automatic worktree-auto creation and user-facing closeout commands;
- broader hosted/team synchronization;
- GUI distribution and support;
- non-Lua extension runtimes.
