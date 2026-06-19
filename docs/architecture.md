# Architecture

imp is a Rust workspace organized around agent runtime responsibilities. The core design keeps provider traffic, tool execution, policy checks, workflow state, sessions, UI, CLI, RPC, and extensions in separate crates while sharing typed runtime models where practical.

## Crates

```text
imp-cli   CLI entrypoint, setup/auth flows, one-shot/headless mode, JSONL RPC mode, ACP stdio server, import/install helpers
imp-core  agent loop, tools, sessions, workflows, policy, recovery, verification, context assembly, storage-facing runtime behavior
imp-llm   provider/model abstraction, streaming, auth helpers, OAuth, model metadata, pricing
imp-lua   shipped Lua extension runtime for tools, slash commands, hooks, and capability policy
imp-tui   terminal UI, interactive app state, rendering, input/event loop, runtime signal handling
```

The repository root package is a source-install shim so `cargo install --path .` works from the workspace root.

## Runtime flow

A typical run follows this path:

1. resolve CLI/TUI/RPC mode and current working directory;
2. load configuration, trust settings, provider credentials, and model metadata;
3. construct an `Agent` with tools, policy context, hooks, session state, and UI/event sinks;
4. build model context from the conversation, selected files, workflow/session hints, and runtime instructions;
5. stream provider output through `imp-llm`;
6. collect tool calls and execute them through `imp-core` tool dispatch under policy/reference-monitor checks;
7. append tool observations back into model context;
8. evaluate continuation policy, workflow obligations, verification gates, and closeout state;
9. persist session/evidence/run artifacts and emit UI/RPC/runtime events.

The agent loop is intentionally runtime-owned: providers stream text/tool calls, but policy, tool effects, recovery checkpoints, workflow obligations, and closeout decisions are enforced locally.

## Provider layer

`imp-llm` hides provider-specific streaming and auth details behind shared model/provider abstractions. The runtime passes a model, request context, request options, and credentials into the provider; the provider returns stream events such as text deltas, thinking deltas, tool calls, message starts/ends, and errors.

Provider-specific concerns belong in `imp-llm`:

- API request/response mapping;
- OAuth helpers and token refresh;
- model metadata and pricing;
- retry classification for provider failures.

Runtime concerns stay outside providers:

- tool execution;
- workflow continuation;
- session persistence;
- reference-monitor decisions;
- UI/RPC event translation.

## Tools and policy

Native tools live in `imp-core/src/tools`. Tool definitions describe parameters, mutability, labels, and execution behavior. Tool execution flows through the agent runtime so policy checks can happen before file mutation, command execution, network access, secret access, or workflow mutation.

Policy-related code is split across configuration, reference monitoring, tool context checks, and workflow closeout enforcement. The important boundary is that tools should not silently bypass write-path checks or user-visible policy decisions.

## Sessions, evidence, and recovery

Session and evidence behavior is file-backed. The runtime records conversation messages, tool results, recovery checkpoints, run evidence, verification gate outputs, and worktree metadata so runs can be inspected after the fact.

Recovery-sensitive tool execution records checkpoints around provider requests, assistant tool calls, tool execution start/end, and tool results entering context. This lets imp distinguish safe retry/recovery paths from side effects that require user review.

## Workflow core

Workflow artifacts live under `.imp/workflows/<id>/` and are parsed/validated by `imp-core/src/workflow`. The model-facing `workflow` tool lives in `imp-core/src/tools/workflow.rs` and currently provides list/show/validate/run/complete_step/update behavior.

The workflow implementation is moving toward a shared service boundary in `imp-core::workflow` so native tools, CLI RPC, ACP/editor integrations, and future app surfaces can share the same operations without duplicating YAML parsing, validation, event append, status reconciliation, or run orchestration.

Current workflow characteristics:

- local file-backed `workflow.yaml`, `events.jsonl`, `results.md`, and artifacts;
- strict validation before accepted mutations;
- command-check execution through `workflow.run`;
- agent-action contracts for non-command steps;
- explicit `complete_step` for successful agent-actionable work;
- durable event records for updates, run checks, completion, and reconciliation.

## CLI, TUI, and RPC surfaces

`imp-cli` owns process entrypoints:

- one-shot/headless prompts;
- TUI launch;
- JSONL RPC mode;
- ACP stdio server;
- setup/auth/import/install helpers.

The TUI in `imp-tui` consumes runtime events, renders messages/tools/status, handles user input, and forwards commands such as cancel/steer/follow-up to the running agent.

JSONL RPC mode is local-process oriented. It accepts prompt/cancel/steer/follow-up commands over stdin and emits structured runtime events over stdout. Workflow RPC methods are planned to call the shared workflow service boundary rather than reimplement workflow logic in the CLI layer.

ACP support is an editor-facing stdio JSON-RPC adapter. It should similarly call shared runtime/workflow operations rather than duplicating agent or workflow behavior.

## Extension runtime

`imp-lua` is the shipped extension runtime. Lua extensions can register tools, slash commands, and hooks through host APIs subject to capability policy.

Extension safety depends on explicit capabilities, policy checks, and clear host/runtime boundaries. Extension code should not become an unreviewed path around native tool policy.

## Planned and experimental surfaces

The following are planned or experimental and should not be described as fully shipped unless separately verified:

- workflow API access through RPC/ACP;
- broader editor integration beyond current ACP scaffolding;
- hosted sync/team collaboration;
- future extension bridges must be documented separately from the shipped Lua runtime.
