# Native tools

imp exposes structured tools to the model. Native tools give the runtime typed parameters, policy metadata, bounded output, and UI-friendly results that shell-only automation cannot provide consistently.

The canonical default registration point is `register_native_tools_with_task_state` in `crates/imp-core/src/builder.rs`.

## Default inventory

| Tool | Purpose |
|---|---|
| `ask_user` | Structured single-select, multi-select, and freeform user questions. |
| `bash` | Shell commands with timeout, cancellation, output bounds, and secret mediation. |
| `edit` | Exact replacement, anchored replacement, and transactional multi-file edits. |
| `git` | Status, diff, log, merge-base, stage, commit, restore, and worktree operations. |
| `read` | Ranged file reads and supported image reads. |
| `scan` | Tree-sitter code structure search, extraction, related symbols, and likely tests. |
| `web` | Web/page and read-only GitHub search. |
| `subagent` | Launch and manage bounded workflow-generated loopr subagent contracts. |
| `task` | Maintain the current session's plan, constraints, step status, and blockers. |
| `workflow` | List, show, validate, run, complete, and update durable workflows. |
| `write` | Explicit file creation or overwrite. |

The `browser` tool is also registered when browser support is enabled in configuration and a compatible Lightpanda runtime is available.

Lua extensions may add tools at runtime. Experimental modules such as `memory`, `prototype`, and TypeScript/Pi compatibility code exist in the repository but are not part of the default native registry.

## Mutability and concurrency

Read-only tool calls may run in parallel. Mutable or side-effecting calls are serialized. Mutation includes file writes, shell commands, git changes, workflow/task updates, subagent launch, and browser actions that alter remote or page state.

Registration and execution policy are separate. A visible tool call can still be denied by role policy, run policy, autonomy, provenance, write-scope checks, browser-input approval, or a hard rail.

## Important contracts

### `task`

`task` is a session-local planning ledger. It supports:

```text
show
plan
update_step
add_constraint
add_blocker
resolve_blocker
```

Runtime-observed file changes, command outcomes, and verification remain authoritative; the model cannot directly write those evidence fields.

### `browser`

Before starting a session, install Lightpanda and ensure `lightpanda version` succeeds. On macOS with Homebrew: `brew install lightpanda-io/browser/lightpanda`. Structured browser lifecycle events are emitted to TUI, JSONL RPC, trace evidence, and durable sessions. The event contract includes session start/stop/failure, navigation, observations, input approval requests and outcomes, completed actions, duration, domain, and per-session sequence. Browser events never include filled values or page content.

Browser sessions are isolated Lightpanda subprocesses. Use `start`, retain the returned `session_id`, then call semantic actions such as `navigate`, `observe`, `markdown`, `extract`, `click`, and `fill`. Call `stop` when finished. Lightpanda does not render screenshots. Browser input defaults to `policy.browser_input = "ask"`. Interactive runs offer once, domain, and session approval scopes; headless runs fail closed unless policy explicitly allows input. Filled values are redacted from approval prompts and records. imp disables Lightpanda telemetry and core dumps, bounds response sizes and operation timeouts, and can block private-network targets.

### TUI settings and health

The TUI Browser tab exposes the Lightpanda binary, session and timeout bounds, maximum response size, `robots.txt`, private-network blocking, and browser-input policy. The health row runs diagnostics asynchronously. Installation displays and confirms the exact package-manager command before execution. Invalid browser settings are rejected before `config.toml` is written.

### Reliability harness

The normal `imp-core` suite runs an offline Lightpanda MCP fault harness covering startup timeout, malformed or mismatched protocol responses, missing tools, bounded and partial responses, process exit, cancellation without retry, tool-level errors, session limits, idle cleanup, sequence isolation, and private-network launch policy.

A deterministic loopback fixture covers redirects, JavaScript forms, semantic observations, markdown extraction, and session sequencing against a real Lightpanda binary:

```bash
LIGHTPANDA_BIN=/path/to/lightpanda \
  cargo test -p imp-core real_lightpanda_deterministic_fixture -- --ignored
```

The fixture explicitly disables private-network blocking only for its isolated loopback server. Production defaults remain unchanged. CI should provide a pinned, checksum-controlled Lightpanda artifact; tests never download a browser binary.

### `subagent`

`subagent` exposes `launch`, `status`, `wait`, `send`, and `cancel`. `launch` accepts only a workflow-generated `SubagentInput`; it validates IDs, objective, allowed context paths, and writable paths against the parent run policy before invoking the separately installed `loopr` executable. It is not a general arbitrary child-process API.

Imp invokes loopr with argument-separated process calls and bounded output/time. It persists the opaque loopr run/thread/session mapping under `.imp/runs/<parent-run-id>/subagents/`, so a later imp process can poll, wait, send, or cancel the same child. Missing loopr, malformed JSON, nonzero exits, stale mappings, and timeout are explicit tool errors. Resource limits are included in the bounded child contract; limits not supported by the loopr CLI are surfaced in structured launch details rather than silently enforced locally.

### `workflow`

The model-facing workflow actions are:

```text
list
show
validate
run
complete_step
update
```

See [Workflows](workflows.md) for lifecycle details.

## Browser runtime

Diagnostic and install commands:

```sh
imp browser doctor
imp browser doctor --json
imp browser install --yes
```

On macOS, the supported manual install path is:

```sh
brew install lightpanda-io/browser/lightpanda
```

Browser sessions run in isolated Lightpanda subprocesses. Start a session, keep its `session_id`, use semantic actions such as navigate/observe/markdown/extract/click/fill, then stop it. Lightpanda does not provide screenshot rendering.

Browser input defaults to `policy.browser_input = "ask"`. Interactive approvals can be scoped once, by domain, or for the session. Headless input fails closed unless policy explicitly allows it. Filled values are redacted from approval prompts and records. imp disables Lightpanda telemetry and core dumps, bounds response sizes/timeouts, and can block private-network targets.

## Display

The TUI renders compact tool cards in the timeline and detailed output in the sidebar. Tool results retain structured `details` for renderers and machine consumers.

## Choosing a tool

Prefer native tools for precise reads/edits, git operations, structural code lookup, workflow/task state, and user questions. Use `bash` for builds, tests, project scripts, package managers, and raw `rg` text search.
