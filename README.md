# imp

Local terminal coding agent in Rust. imp runs through a TUI, one-shot prompts, or a JSONL RPC protocol. It uses structured tools, durable sessions, and file-backed workflows for planned, inspectable development work.

## Demo

<video src="docs/assets/imp-demo.mp4" controls muted playsinline width="100%"></video>

If your Markdown viewer does not render embedded video, open [`docs/assets/imp-demo.mp4`](docs/assets/imp-demo.mp4) directly.

## Install

Homebrew:

```bash
brew tap kfcafe/tap && brew install imp
```

From a checkout with a recent stable Rust toolchain:

```bash
cargo install --path .
imp install-local
```

## Quickstart

```bash
imp                           # TUI
imp tui                       # TUI, explicit
imp -p "Summarize this repo"   # one-shot prompt
imp --mode rpc                # JSONL RPC protocol
imp -c                        # continue latest session
```

## Features

Runtime features:

- terminal UI
- one-shot prompt mode
- JSONL RPC mode
- provider-flexible model runtime
- structured native tools
- durable JSONL sessions
- context compaction
- workflow-backed planning and verification
- trace/evidence artifacts
- OS-backed secret storage
- runtime policy for tools, writes, autonomy, and hooks

Workflow features:

Workflows are local project records for multi-step work. They keep the plan, execution state, verification, and closeout notes together instead of leaving them only in the conversation.

- workflow artifacts under `.imp/workflows`
- YAML workflow schema
- schema-checked workflow updates
- append-only workflow events
- next-action selection
- acceptance/check tracking
- results and closeout records

Extension features:

- Lua tools
- Lua slash commands
- Lua hooks
- extension capability policy
- preview Rust SDK

## Local data and provider traffic

Local execution:

- agent runtime
- TUI and RPC surfaces
- tool execution
- file reads/writes/edits
- shell commands
- git operations
- workflow files and event logs
- session JSONL records
- Lua hooks/extensions

Provider traffic:

- prompts
- selected context
- tool observations used for a turn
- web-search/read requests when web tools are used

Secret values are stored in the OS credential store. `~/.imp/auth.json` stores metadata.

| Path | Contents |
|---|---|
| `~/.config/imp/config.toml` | user config |
| `<project>/.imp/config.toml` | project config |
| `~/.imp/auth.json` | auth metadata |
| `.imp/workflows/` | workflow YAML, events, results, artifacts |

## Providers

Provider families:

- Anthropic
- OpenAI / ChatGPT
- Google
- OpenAI-compatible APIs
- Moonshot / Kimi
- Z.AI / GLM
- DeepSeek
- Groq
- Cerebras
- xAI
- Mistral
- Together
- OpenRouter
- Fireworks

API-key configuration:

```bash
export ANTHROPIC_API_KEY=...
export OPENAI_API_KEY=...
export GOOGLE_API_KEY=...
export OPENROUTER_API_KEY=...
```

Credential commands:

```bash
imp login
imp login openai
imp login kimi
imp secrets list
imp secrets doctor
```

## Tools

| Tool | Feature |
|---|---|
| `read` | ranged file/image reads |
| `write` | file creation/overwrite |
| `edit` / `multi_edit` | exact and transactional edits |
| `bash` | shell commands with timeout/cancellation |
| `git` | status, diff, log, stage, commit, restore, worktrees |
| `scan` | tree-sitter code search/extraction |
| `web` | web/GitHub search and page reads |
| `browser` | stateful semantic browsing through Lightpanda |
| `ask_user` | structured user prompts |
| `workflow` | workflow list/show/validate/run/update |

The model uses native tools instead of relying only on shell commands. This gives imp narrower, policy-checkable operations for common development tasks.

Tool execution rules:

- read-only tools can run in parallel
- mutable tools are serialized
- runtime policy checks tool visibility and execution
- write-path policy checks file mutations
- autonomy policy controls unattended action level

## Workflows

Workflows are YAML-backed project artifacts for work that needs more structure than a single prompt. A workflow can describe the plan, the required context, the execution steps, the checks, verification results, and the evidence needed to close the work.

Workflow root:

```text
.imp/workflows/<id>/
├── workflow.yaml
├── events.jsonl
├── results.md
└── artifacts/
```

Workflow schema fields:

- metadata
- parent workflow reference
- settings
- goal/user value
- non-goals
- acceptance criteria
- context requirements
- steps
- dependencies
- checks
- verification evidence
- workers
- results
- closeout rules

Workflow tool actions:

```text
list
show
validate
run
update
```

Workflow lifecycle:

```text
inspect → validate → run → update → verify → review → closeout
```

Update behavior:

- validates the edited workflow before writing
- rejects oversized workflow YAML before parsing
- opens/preflights `events.jsonl`
- writes allowed status/path changes
- appends `events.jsonl`
- rejects invalid status values
- keeps workflow state file-backed and reviewable

Workflow command surface:

- use the native `workflow` tool for list/show/validate/run/update from agent turns
- use workflows as the durable replacement for older `/plan`, `/run`, `/debug`, `/review`, and `/verify` task-type commands
- current runner selects and records next actions; executable build-step orchestration is tracked as follow-up work

Current storage is local and file-backed. API-addressable workflows are planned.

## Sessions

Session records:

- messages
- tool calls
- tool results
- usage metadata
- cost metadata
- branch metadata
- compaction entries
- checkpoint/recovery records

Evidence surfaces:

- `--verify` commands
- trace events
- evidence packets
- final outcomes: `DONE`, `DONE_WITH_CONCERNS`, `BLOCKED`, `NEEDS_CONTEXT`

```bash
imp evidence list
imp evidence latest
```

## TUI controls

| Input | Feature |
|---|---|
| `/` | command palette |
| `@` | file context attachment |
| `Ctrl+L` | model selector |
| `Shift+Tab` | thinking level |
| `/compact` | context compaction |
| `/model` | model selector |
| `/settings` | settings editor |
| `/secrets` | credential manager |
| `/login` | provider OAuth login |
| `/new` / `/resume` | session lifecycle |
| `/loop` / `/stop` | continue or stop active work |

## RPC mode

`--mode rpc` starts a JSON-lines stdin/stdout protocol for host applications. It is the non-TUI integration surface for embedding imp in another process.

Input command types:

```text
prompt
steer
followup
cancel
```

Output includes agent, tool, stream, runtime event, and runtime state payloads. `--runtime-json` emits the shared runtime event/state shape alongside legacy JSON fields.

```bash
imp --mode rpc --runtime-json
```

## Policy

```bash
IMP_MODE=reviewer imp -p "inspect this diff"
imp -p "inspect this diff" --deny-tool bash --deny-write '**'
imp -p "fix the failing test" --allow-write crates/imp-core --verify "cargo test -p imp-core"
```

Policy controls:

```bash
--allow-tool read --allow-tool git
--deny-tool bash
--allow-write crates/imp-core
--deny-write '**/*.lock'
--autonomy safe
--verify "cargo test"
```

Autonomy modes:

```text
suggest
safe
local-auto
worktree-auto
allow-all-local
allow-all
ci
```

## Configuration

Precedence:

1. built-in defaults
2. `~/.config/imp/config.toml`
3. `<project>/.imp/config.toml`
4. environment variables
5. CLI flags

Browser diagnostics and installation:

```bash
imp browser doctor
imp browser doctor --json
imp browser install --yes
```

The doctor checks resolved configuration, executable version, MCP compatibility, and required tools. Installation uses Homebrew on macOS and requires explicit confirmation. Direct release downloads are not used because upstream does not publish checksum assets.

## Browser settings

The TUI settings overlay includes a **Browser** tab for Lightpanda health, confirmed package-manager installation, enablement, binary path, session and timeout bounds, response size, `robots.txt`, private-network blocking, and browser-input policy. The health row runs the same asynchronous diagnostic API used by `imp browser doctor`; invalid values are rejected before saving.

Structured browser lifecycle events are available in JSONL RPC as `browser_event` records and in the canonical runtime stream as `browser_updated`. Payloads contain sanitized lifecycle metadata rather than filled values or page content.

```toml
model = "sonnet"
thinking = "medium"
max_turns = 100
max_tokens = 2048

[browser]
enabled = true
# binary = "/path/to/lightpanda"
max_sessions = 2
timeout_ms = 30000
obey_robots = true
block_private_networks = true

[policy]
browser_input = "ask"

[web]
search_provider = "exa"

[ui]
notify_on_agent_complete = true
```

## Extensions

Stable extension runtime: Lua.

Load paths:

```text
~/.imp/lua/
<project>/.imp/lua/
```

New extensions can be manifest-backed packages:

```text
~/.imp/lua/github/
  manifest.lua
  init.lua
```

```lua
-- manifest.lua
return {
    name = "github",
    version = "0.1.0",
    description = "GitHub helper commands",
    commands = {
        { name = "review-pr", description = "Review the current PR" },
    },
}
```

```lua
-- init.lua
local imp = require("imp")

imp.command("review-pr", {
    description = "Review the current PR",
    run = function(args) return "Reviewing " .. (args or "current PR") end,
})
```

Manifest-backed commands are invoked through the extension slash namespace:

```text
/x github.review-pr
```

Extension features:

- tools
- slash commands
- hooks
- capability policy for shell/filesystem/HTTP/secrets/native tools

```lua
imp.register_command("greet", {
    description = "Say hello",
    handler = function(args) return "Hello, " .. (args or "world") end
})
```

## Rust SDK

Preview API: `imp_core::sdk`.

```rust,no_run
use imp_core::sdk::{AgentEvent, ImpSession, Result, SessionOptions};

#[tokio::main]
async fn main() -> Result<()> {
    let mut session = ImpSession::create(SessionOptions {
        cwd: std::env::current_dir()?,
        ..Default::default()
    })
    .await?;

    session.prompt("Summarize this repository.").await?;

    while let Some(event) = session.recv_event().await {
        if let AgentEvent::AgentEnd { .. } = event {
            break;
        }
    }

    session.wait().await
}
```

## Crates

```text
imp-cli   CLI entrypoint, TUI launch, one-shot/headless/RPC modes
imp-core  agent loop, tools, sessions, workflows, policy, SDK
imp-llm   providers, streaming, auth, model metadata
imp-lua   Lua extension runtime
imp-tui   terminal UI
```

## Status

Active:

- TUI
- one-shot prompts
- JSONL RPC mode
- native tools
- durable sessions
- file-backed workflows
- verification/evidence
- provider auth
- OS-backed secrets
- policy controls
- Lua extensions
- Rust SDK preview

Preview/planned:

- executable workflow runner for build-step orchestration
- MCP planned
- `.imp/agents` planned
- ACP editor adapter scaffold remains internal/out-of-scope for 0.3.0 unless separately verified
- hosted sync/team collaboration planned
- workflow API planned

Compatibility/legacy:

- TypeScript/Pi extension compatibility is experimental and not a shipped extension surface

## Technical docs

- [Docs index](docs/index.md)
- [Workflows](docs/workflows.md)
- [ACP editor adapter scaffold](docs/acp.md) — internal/out-of-scope for 0.3.0 unless separately verified.
- [RPC protocol](docs/rpc.md)
- [Native tools](docs/tools.md)
- [Runtime policy](docs/policy.md)
- [Sessions and evidence](docs/sessions.md)
- [Lua extensions](docs/extensions-lua.md)
- [Architecture](docs/architecture.md)

## Development

```bash
cargo test --workspace --all-targets
cargo bench -p imp-core --bench core_hot_paths
```

```bash
bash tools/run-leaks.sh
bash tools/run-miri.sh
bash tools/run-asan.sh
bash tools/run-tsan.sh
bash tools/run-stress.sh
```

## License

MPL-2.0. See [LICENSE](LICENSE).
