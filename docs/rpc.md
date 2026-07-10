# RPC protocol

`imp --mode rpc` runs imp as a JSON-lines stdin/stdout process for host applications.

Primary implementation area:

- `crates/imp-cli/src/lib.rs`

## Start

```bash
imp --mode rpc
imp --mode rpc --runtime-json
imp --mode rpc --session .imp/host-session.jsonl
```

`--runtime-json` emits the shared runtime event/state shape alongside legacy JSON fields.
With `--session PATH`, RPC opens the existing transcript at that exact path or
creates it there. Relaunching with the same path resumes completed conversation
history, including consumed follow-up messages.

RPC owns stdin immediately; piped stdin is never consumed as a one-shot prompt.
After configuration and session initialization, stdout emits:

```json
{"type":"rpc_ready","protocol":"imp-rpc","version":1,"capabilities":["durable_sessions","prompt","followup","steer","cancel"]}
```

Hosts must read until `rpc_ready` before sending the first command, validate the
protocol version, and require the capabilities they use. Unknown additional
capabilities are forward-compatible. Any earlier additive events should be
preserved or ignored according to the host policy.

After configuration and protocol initialization, stdout emits a readiness barrier:

```json
{"type":"rpc_ready","protocol":"imp-rpc","version":1,"capabilities":["prompt","followup","steer","cancel"]}
```

Hosts must wait for `rpc_ready`, require protocol version `1`, and verify every
capability they depend on before sending commands. Unknown capabilities are
forward-compatible. Version `1` does not promise durable command delivery,
acknowledgments, replay, or restart of an in-flight run.

## Input commands

Each input line is a JSON object with a `type` field.

```json
{"type":"prompt","content":"Summarize this repository."}
{"type":"steer","content":"Prefer small reversible changes."}
{"type":"followup","content":"Now run the tests."}
{"type":"cancel"}
```

Command types:

| Type | Behavior |
|---|---|
| `prompt` | starts a run or queues the prompt if a run is active |
| `steer` | sends steering text to the active run |
| `followup` | queues a follow-up prompt |
| `cancel` | cancels the active run |

`prompt`, `steer`, and `followup` require `content`.

## Output

RPC output is also JSON-lines. Events include agent lifecycle, streaming text, tool calls, tool results, policy checks, recovery checkpoints, evidence writes, and runtime state updates.

Host applications should treat unknown event fields as forward-compatible
additions and use `rpc_ready` as the initialization barrier.

## Runtime JSON

With `--runtime-json`, output includes normalized runtime event/state payloads. Use this mode for new host integrations when possible.

## Host integration notes

- Read stdout line-by-line and wait for `rpc_ready` before sending commands.
- Write one JSON command per stdin line.
- Preserve and reuse the same `--session` path to resume after process restart.
- Do not assume a single prompt produces a single output message.
- Handle cancellation and queued follow-ups explicitly.
- Treat tool output as structured event data, not plain terminal text.
- Keep the process cwd scoped to the project being operated on.
