# imp crate boundary plan

Status: proposal
Last updated: 2026-06-02

## Why this exists

`imp-core` is currently the center of the runtime, tool registry, sessions, workflow machinery, policy, storage, code intelligence, and many native tools. That has been practical while imp is small, but it creates the same failure mode that larger agent codebases run into: the central crate becomes the place every new capability lands, and every consumer inherits the full dependency graph.

This plan keeps the split intentionally modest. The goal is not to create a crate for every subsystem. The goal is to move heavyweight implementation dependencies out of `imp-core` while keeping product-facing behavior cohesive.

## Current pressure points

Recent static audit snapshot:

```text
imp workspace:
  ~8 Rust packages
  ~199 Rust files
  ~115k Rust code lines

largest crates:
  ~74k  imp-core
  ~31k  imp-tui
  ~12k  imp-llm
  ~8.5k imp-cli
  ~2.8k imp-lua
```

`imp-core` is already doing a lot:

```text
~25.5k  tools/
~11.5k  agent/
~6.6k   workflow/
~2.8k   session.rs
~2.1k   typescript_extensions/
~1.9k   reference_monitor.rs
~1.8k   imp_session.rs
~1.8k   system_prompt.rs
~1.7k   config.rs
```

Heavy dependencies currently owned directly by `imp-core` include:

- all tree-sitter grammars;
- `rusqlite` with bundled SQLite;
- `jsonschema`;
- `readability-rust`;
- `reqwest`;
- broad Tokio features through workspace defaults.

These are useful capabilities, but they should not all define what it means to depend on the core agent runtime.

## Design principles

1. **Keep product-facing capabilities together.**
   The model-facing `scan` tool should stay with the other native tools. The tree-sitter engine underneath it can move out.

2. **Extract heavyweight engines, not every feature.**
   Move tree-sitter parsing and SQLite storage first. Do not prematurely split every tool family.

3. **Keep `imp-core` as the domain/runtime center.**
   A smaller `imp-core` is good; a hollow `imp-core` with twenty tiny crates around it is not the target.

4. **Create a composition crate for the binary.**
   `imp-cli` should not have to depend on `imp-tui` just so the installed binary can launch the TUI.

5. **Prefer compatibility façades during migration.**
   Re-export moved APIs from `imp-core` temporarily if that keeps downstream churn low.

## Professional engineering goals

The crate split is only one part of making imp feel professionally engineered. The broader goal is to make the system's boundaries, safety model, and verification story visible to maintainers and embedders.

Target qualities:

- **Legible composition:** the final user-facing binary composes CLI, TUI, Lua, runtime, state, and tools deliberately instead of inheriting them accidentally.
- **Small core contract:** `imp-core` exposes domain concepts and stable SDK seams while heavyweight implementation engines live in narrower crates.
- **Headless hostability:** RPC/worker use cases can run without terminal UI dependencies and, eventually, without optional engines they do not need.
- **Durable state discipline:** session, workflow, memory, trace, evidence, and schema formats have explicit ownership and compatibility expectations.
- **Security-reviewable paths:** shell execution, file mutation, secrets, provider traffic, Lua extensions, and policy decisions are easy to locate and test.
- **Measured dependencies:** dependency growth is intentional, especially for native, parser, database, network, UI, and extension-runtime crates.
- **Documented decisions:** architectural changes leave a short decision trail, not just a diff.

## Boundary decision table

Use this table when deciding where new behavior belongs.

| Behavior | Preferred home | Notes |
|---|---|---|
| Top-level `imp` binary mode dispatch | `imp-bin` | Compose TUI, CLI, Lua, runtime, and feature profiles here. |
| One-shot prompt, RPC, login/setup commands | `imp-cli` | Avoid direct TUI dependency for headless paths. |
| Terminal rendering, input, overlays, app view state | `imp-tui` | TUI should consume runtime events rather than own agent logic. |
| Turn loop, cancellation, retries, tool scheduling, runtime events | `imp-runtime` once seam is clear | Extract only when it avoids cycles and vague trait indirection. |
| Domain config, policy concepts, SDK façade, workflow domain | `imp-core` | Keep core intentional and avoid heavy implementation dependencies. |
| Native model-facing tools and registry | `imp-tools` | Keep `scan`, `workflow`, `read`, `write`, `edit`, `bash`, etc. cohesive. |
| Tree-sitter parsing, symbol extraction, code search engine | `imp-codeintel` | `scan` remains a tool; code intelligence is the engine underneath. |
| Sessions, indexes, SQLite, memory, traces, evidence | `imp-state` | Own durable formats and migrations/compatibility. |
| Provider streaming, model metadata, auth/client abstractions | `imp-llm` | Split auth later only if hostability demands it. |
| Lua tool/command/hook runtime | `imp-lua` | Keep extension capability policy explicit and reviewable. |

## Proposed near-term crate shape

```text
imp-bin
├── imp-cli
├── imp-tui
├── imp-lua
└── imp-core
    ├── imp-llm
    ├── imp-runtime
    ├── imp-state
    ├── imp-tools
    └── imp-codeintel
```

This is intentionally less spread than a maximal split. In particular, there is no immediate `imp-policy`, `imp-web`, `imp-workflow`, `imp-tool-api`, or `imp-types` crate unless later pressure proves they are needed.

## Crate responsibilities

### `imp-core`

Role: agent domain, high-level orchestration, and stable SDK surface.

Keep in `imp-core` initially:

```text
agent-facing domain types
builder-facing configuration glue
context assembly APIs
compaction policy
system prompt assembly
workflow domain, initially
workflow review, initially
policy/trust/guardrail domain, initially
public SDK façade
error/display helpers
```

`imp-core` should eventually stop directly depending on:

```text
tree-sitter-*
rusqlite
syntect
ratatui
crossterm
mlua
readability-rust, unless web remains core
jsonschema, if workflow schema validation moves
```

`imp-core` can continue to re-export selected APIs from split crates while the migration settles.

### `imp-runtime`

Role: live execution layer for turns, tool dispatch, runtime events, cancellation, concurrency, and session execution.

This split is worth considering because `runtime` is a distinct concept in the project vocabulary: it is the live execution layer, while state/session files are durable graph/state.

Good candidates for `imp-runtime`:

```text
runtime.rs
parts of imp_session.rs that own live execution
agent/run_loop.rs
agent/tool_execution.rs
agent/events.rs
agent/loop_state.rs
agent/loop_policy.rs
agent/recovery.rs
agent/autonomy.rs
agent/turn_assessment.rs
workflow_integration runtime adapters
runtime event/state types used by TUI/RPC
```

Keep in `imp-core` rather than `imp-runtime`:

```text
public builder façade, at least initially
system prompt/domain policy
high-level agent API surface
stable SDK re-exports
```

Possible final relationship:

```text
imp-core -> imp-runtime
imp-runtime -> imp-tools
imp-runtime -> imp-state
imp-runtime -> imp-llm
```

Rationale:

- TUI and RPC surfaces often need runtime events, cancellation, and execution state without owning the whole core domain.
- Tool execution policy and scheduling are live-runtime concerns.
- Durable sessions/storage should not be mixed with live turn execution.

Caution:

Do not extract `imp-runtime` before the seam is clear. If moving it requires inventing lots of abstract traits just to break cycles, wait. The safer first splits are `imp-bin`, `imp-codeintel`, and `imp-state`.

### `imp-state`

Role: durable local state and persistence.

Move here:

```text
storage.rs
session.rs
session_index.rs
memory.rs
trace.rs
usage.rs
evidence.rs
run_evidence.rs
eval_candidate.rs
eval_candidate_closeout.rs
```

Maybe move later:

```text
imp_session.rs, only if the durable-session parts can be separated from live runtime orchestration
```

Own dependencies:

```text
rusqlite
chrono
uuid
serde
serde_json
walkdir, if needed for discovery/indexing
```

This isolates bundled SQLite from crates that only need agent runtime types.

### `imp-codeintel`

Role: reusable code intelligence engine.

Move here:

```text
codeintel/
repo_intelligence.rs, if it is primarily indexing/search engine behavior
language detection/parser registration
symbol extraction/search primitives
```

Own dependencies:

```text
tree-sitter
tree-sitter-* grammars
rayon, if parallel scan/indexing stays here
project-detect, if project/language detection belongs here
ignore/walkdir, if traversal stays in the engine
```

Do **not** move the model-facing scan tool here. The scan tool remains part of `imp-tools` and calls `imp-codeintel`.

### `imp-tools`

Role: native, model-facing tools and their registry.

Move here as one cohesive crate:

```text
tools/mod.rs
tools/ask.rs
tools/bash.rs
tools/edit.rs
tools/git.rs
tools/lua.rs
tools/memory.rs
tools/multi_edit.rs
tools/query.rs
tools/read.rs
tools/scan/
tools/code_intel.rs
tools/shell.rs
tools/subagent.rs
tools/web/
tools/workflow.rs
tools/write.rs
tools/prototype.rs
```

`imp-tools` can depend on:

```text
imp-core, for domain/runtime-facing types during the first migration
imp-runtime, if runtime is extracted
imp-state
imp-codeintel
imp-llm
```

Avoid splitting `imp-tools` into many small crates initially. Keep the model-facing tool surface coherent.

Longer-term, only split a tool family if it creates real dependency pressure. For example, if web/readability dependencies become painful, consider `imp-web` later; do not create it preemptively.

### `imp-cli`

Role: noninteractive CLI commands, setup/auth commands, one-shot prompt mode, JSONL RPC mode, and command dispatch helpers.

Near-term goal:

```text
imp-cli should not depend on imp-tui by default.
```

`imp-cli` should be usable for headless/RPC modes without pulling in:

```text
ratatui
crossterm
syntect
TUI view code
```

### `imp-bin`

Role: final installed `imp` binary and feature composition.

Current shape:

```text
root imp-install -> imp-cli -> imp-tui
```

Target shape:

```text
imp-bin -> imp-cli
imp-bin -> imp-tui, feature-gated or normal default feature
imp-bin -> imp-lua, feature-gated if desired
```

`imp-bin` owns runtime construction and top-level mode dispatch:

```text
imp            -> TUI by default
imp tui        -> TUI explicitly
imp -p ...     -> one-shot prompt via imp-cli
imp --mode rpc -> JSONL RPC worker via imp-cli
imp login      -> auth/setup via imp-cli
```

Potential feature shape:

```toml
[features]
default = ["tui", "lua", "codeintel", "workflow", "web"]
tui = ["dep:imp-tui"]
lua = ["dep:imp-lua"]
minimal = []
```

This makes lean worker builds possible without compromising the normal user install.

### `imp-lua`

Role: Lua extension runtime.

Keep as a separate crate. It may depend on `imp-tools` and `imp-core`/`imp-runtime`, but minimal/headless builds should eventually be able to omit `mlua` if extension support is disabled.

### `imp-llm`

Role: provider and model runtime.

Keep as-is for now. Later, consider splitting OS credential storage into `imp-auth` if hostability demands it, but this is not a near-term priority.

## Recommended migration order

### 1. Add `imp-bin` and remove default `imp-cli -> imp-tui` coupling

This is the cleanest first step because it improves layering without moving deep runtime internals.

Acceptance criteria:

- installed `imp` behavior is unchanged;
- `imp tui` still launches the TUI;
- one-shot and RPC modes can be implemented without direct TUI dependency in `imp-cli`;
- root install shim continues to work or delegates to `imp-bin`.

### 2. Extract `imp-codeintel`, while keeping scan in `imp-tools` / current tools module

Move the engine, not the tool UX.

Acceptance criteria:

- `scan` tool behavior unchanged;
- tree-sitter grammar dependencies move out of `imp-core`;
- `imp-core` calls code-intelligence through a small API;
- no broad tool registry changes yet.

### 3. Extract `imp-state`

Move storage/session/index/memory/trace persistence.

Acceptance criteria:

- session read/write behavior unchanged;
- `rusqlite` moves out of `imp-core`;
- TUI/session history behavior still works;
- existing session formats remain compatible.

### 4. Consider `imp-runtime`

After state and code intelligence are separated, inspect whether live execution has a natural seam.

Good sign:

- runtime/event/cancellation/tool-execution code can move without forcing many new abstraction traits.

Bad sign:

- extraction creates cyclic dependencies or vague interfaces.

If the seam is good, move runtime and turn-loop execution pieces into `imp-runtime`.

### 5. Extract `imp-tools`

Once `imp-runtime`, `imp-state`, and `imp-codeintel` boundaries are clearer, move all native tools together.

Acceptance criteria:

- tool registration remains simple;
- `scan` remains a normal model-facing tool;
- no unnecessary split into `imp-tools-fs`, `imp-tools-git`, etc.;
- runtime policy still applies before and during tool execution.

## Definition of done for migration PRs

Each boundary migration should preserve behavior first and reduce coupling second. A migration is not done merely because files moved.

For each migration PR, include:

1. **Boundary statement**
   - What moved.
   - What stayed.
   - What dependency pressure, ownership ambiguity, or hostability problem the move reduces.

2. **Behavior preservation evidence**
   - The narrowest relevant `cargo check` or `cargo test` command.
   - Any targeted tests for moved behavior.
   - A note on user-visible behavior that should remain unchanged.

3. **Dependency evidence**
   - Before/after direct dependencies for the affected crates when the point of the change is dependency reduction.
   - Confirmation that `imp-core` no longer owns the moved heavyweight dependency when applicable.

4. **API compatibility decision**
   - Whether `imp-core` re-exports moved APIs temporarily.
   - Whether downstream imports should migrate immediately or over a compatibility window.
   - Any public SDK impact.

5. **Format compatibility decision**
   - Whether session, workflow, memory, trace, evidence, config, or extension formats changed.
   - If changed, migration/backward compatibility behavior.
   - If unchanged, say so explicitly.

6. **Security-sensitive path check**
   - If the move touches shell, file mutation, secrets, Lua, provider traffic, policy, or workflow verification, identify the policy checks that still apply.

## First concrete migrations

### Migration A: `imp-bin`

Goal: separate final binary composition from headless CLI behavior.

Implementation outline:

```text
create crates/imp-bin
move final `imp` binary entrypoint there
make root install shim delegate to imp-bin or depend on imp-bin
keep imp-cli responsible for headless/RPC/setup/auth commands
remove default imp-cli -> imp-tui dependency
```

Done when:

- `cargo install --path .` still installs an `imp` binary;
- `imp` still launches the TUI by default;
- `imp tui`, `imp -p ...`, and `imp --mode rpc` still route to the same behavior;
- `cargo check -p imp-cli` does not require `imp-tui` as a normal dependency;
- `cargo check -p imp-bin` verifies composition.

### Migration B: `imp-codeintel`

Goal: move parser/search engine dependencies out of `imp-core` while keeping the scan tool in the model-facing tool surface.

Implementation outline:

```text
create crates/imp-codeintel
move codeintel engine modules
move tree-sitter grammar dependencies
keep tools/scan in imp-tools or current tools module during transition
call imp-codeintel from the scan tool
```

Done when:

- `scan` tool behavior and output shape are unchanged;
- tree-sitter grammar dependencies move from `imp-core` to `imp-codeintel`;
- `cargo test -p imp-codeintel`, or the nearest available code-intel tests, pass;
- the crate that owns `scan` still exposes one cohesive model-facing tool registry.

### Migration C: `imp-state`

Goal: isolate durable storage and bundled SQLite from the core domain/API crate.

Implementation outline:

```text
create crates/imp-state
move session storage, session index, memory, traces, usage, evidence persistence
keep live execution orchestration out of imp-state
re-export compatibility APIs from imp-core if needed
```

Done when:

- existing session files still load;
- new session files preserve the expected JSONL/tree structure;
- workflow/evidence paths still resolve the same way;
- `rusqlite` no longer needs to be a direct `imp-core` dependency;
- TUI session history and headless continue/resume paths still work.

### Migration D: `imp-runtime`

Goal: separate live turn execution from domain types and durable state only if the seam is natural.

Implementation outline:

```text
inspect agent loop, runtime events, imp_session, cancellation, recovery, tool scheduling
move only cohesive live-execution code
avoid trait indirection solely to break cycles
```

Done when:

- TUI and RPC consume the same runtime event stream as before;
- cancellation, retry, tool execution, and compaction behavior are preserved;
- policy checks still happen before sensitive actions;
- the move does not introduce broad cyclic imports or vague adapter traits.

## Feature profiles and dependency policy

A professional imp build should make optional capability cost explicit. The normal user install can still be full-featured, but internal builds should be able to select narrower profiles.

Candidate profiles:

```text
full user install:
  TUI + CLI + RPC + Lua + workflows + code intelligence + web + SQLite state

headless worker:
  CLI + RPC + runtime + state + tools, no TUI

minimal embedded runtime:
  runtime + LLM abstractions + selected tools, no TUI, no Lua, no web, no tree-sitter unless requested

workflow runner:
  runtime + state + workflow domain/tooling + verification, TUI optional
```

Dependency rules to preserve those profiles:

- `imp-core` should not directly own UI, SQLite, tree-sitter grammars, Lua, or syntax-highlighting dependencies once the relevant split exists.
- `imp-cli` should not depend on `imp-tui` by default.
- `imp-tools` may depend on engines such as `imp-codeintel`, but tool behavior should remain model-facing and cohesive.
- Network-heavy, parser-heavy, database-heavy, or native dependencies need an owner crate and a reason.
- Generated artifacts, schemas, and durable data formats should document their regeneration command or source of truth.

## Professionalization follow-ups

These are not prerequisites for the crate split, but they make the architecture durable.

### Architecture decision records

Add short ADRs under `docs/adr/` for decisions that affect long-lived boundaries. Useful first ADRs:

```text
0001-crate-boundaries.md
0002-runtime-vs-state.md
0003-tools-and-codeintel-boundary.md
0004-binary-composition.md
0005-workflow-artifacts-as-durable-state.md
```

Each ADR should cover context, decision, consequences, and current status. Keep them short enough that future agents will actually read them.

### Architecture docs

Keep `docs/architecture.md` aligned with this plan as implementation lands. It should eventually show:

```text
user input / host command
  -> imp-bin
  -> imp-cli or imp-tui
  -> imp-runtime
  -> imp-llm + imp-tools
  -> imp-state
```

### Test strategy

Add or maintain a test strategy that names the important layers:

```text
unit:       config parsing, policy decisions, pure state transitions
integration: tool execution, session persistence, workflow mutation, compaction
contract:  RPC protocol, extension API, provider stream events
snapshot:  TUI rendering and user-visible terminal output
smoke:     fake-provider headless run in a temp workspace
```

### Security model

Document the security-sensitive seams:

```text
shell execution
file reads/writes/edits
secret storage
provider traffic
Lua extension capabilities
workflow/run verification
policy and autonomy modes
```

This should be close to the code paths that enforce policy, not just high-level product prose.

### Boundary checks

Consider lightweight scripts that report:

```text
crate line counts
normal dependency counts
heavy dependencies by owner crate
duplicate dependencies
largest source files
feature graph for imp-bin profiles
```

A first version can be simple shell/Python rather than a full tool. Useful commands:

```bash
cargo metadata --format-version 1 --no-deps
cargo tree -p imp-cli --edges normal --duplicates
cargo tree -p imp-tui --edges normal --duplicates
find crates -name '*.rs' -print | xargs wc -l | sort -nr | head
```

These reports do not need to block every change, but they make dependency drift visible before imp repeats the central-core bloat pattern.

## Dependency cleanup to do along the way

### Narrow Tokio features

Current workspace uses broad Tokio features. Move toward crate-specific features instead of `full` everywhere.

### Align duplicate `crossterm`

Audit currently showed both direct and transitive crossterm versions. Prefer aligning the direct TUI dependency with the version expected by the current ratatui stack.

### Isolate duplicate `reqwest` pressure

`jsonschema` currently pulls a newer reqwest family than direct app dependencies. Moving schema validation into workflow-specific code may reduce how much of the graph sees that duplication.

### Keep syntax highlighting TUI-only

`syntect` belongs in `imp-tui`, not core or CLI.

### Keep Lua optional at composition time

Lua is a shipped extension feature, but minimal worker builds should not have to link vendored Lua if they do not support extensions.

## What not to split yet

Avoid creating these unless there is concrete pressure:

```text
imp-policy
imp-web
imp-workflow
imp-tool-api
imp-types
imp-auth
imp-tools-fs
imp-tools-git
imp-tools-web
```

They may become useful later, but the first migration should stay legible.

## Desired end state

A normal installed build can still include the rich feature set:

```text
TUI + CLI + RPC + Lua + workflows + code intelligence + web + durable sessions
```

But the architecture should also support leaner internal builds:

```text
headless/RPC worker without TUI
agent runtime without tree-sitter grammars
runtime tests without bundled SQLite if not needed
CLI setup/auth without TUI rendering stack
```

The strategic objective is to keep imp from repeating the common agent-codebase failure mode where the central runtime crate becomes the transitive owner of every product capability.
