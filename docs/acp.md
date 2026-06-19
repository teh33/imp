# ACP editor adapter

imp has an early Agent Client Protocol (ACP) stdio adapter behind:

```sh
imp acp
```

ACP is a JSON-RPC protocol for editor/agent integration. The adapter is being built as a sibling to imp's existing `--mode rpc` JSONL worker protocol; the two protocols are intentionally not wire-compatible.

## Current status

This implementation is scaffold-level and suitable for protocol/client smoke testing, not daily editor use yet.

Implemented now:

- `imp acp` subcommand.
- newline-delimited JSON-RPC over stdio.
- `initialize` handshake for ACP protocol version 1.
- conservative capability advertisement.
- `session/new` with absolute `cwd` validation.
- durable imp session creation for ACP sessions.
- `session/prompt` request parsing and completion response shape.
- `session/cancel` notification state handling for the current scaffold.
- initial imp event to ACP `session/update` mapping helpers.

Not implemented yet:

- live `Agent::run` turn execution from ACP prompts.
- real assistant text streaming from provider calls.
- `session/request_permission` bridge for imp UI/tool approvals.
- full policy-denial-to-ACP UX.
- `session/load` / `session/resume` methods, although durable session lookup helpers exist.
- client-supplied server configuration.
- image/audio prompt content.
- ACP registry metadata.

## Transport rules

`imp acp` uses ACP's stdio transport:

- stdin: one JSON-RPC 2.0 message per line.
- stdout: one JSON-RPC 2.0 message per line.
- stdout must contain only ACP JSON-RPC messages.
- diagnostics should go to stderr.

## Minimal smoke test

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"smoke"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"'"$PWD"'"}}' \
  | imp acp
```

Expected behavior:

- response 1 includes `result.protocolVersion: 1` and `agentInfo.name: "imp"`.
- response 2 includes a durable imp `sessionId`.

## Editor configuration shape

Editors that support custom ACP agents generally need a command and args. Use:

```json
{
  "command": "imp",
  "args": ["acp"]
}
```

You can pass normal imp model/provider options before the subcommand when needed:

```json
{
  "command": "imp",
  "args": ["--provider", "anthropic", "--model", "claude-sonnet-4", "acp"]
}
```

## Troubleshooting

- If the editor reports invalid JSON, make sure nothing writes logs to stdout in ACP mode.
- If `session/new` fails, verify the client sends an absolute `cwd` and does not include unsupported server configuration.
- If prompt execution appears stubbed, that is expected in the current scaffold. Live agent turn wiring is the next implementation step.
- If auth/model setup fails once live turns are connected, configure credentials with `imp login <provider>` or pass the appropriate provider/API key options.
