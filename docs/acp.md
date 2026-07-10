# ACP editor adapter

imp has an early Agent Client Protocol (ACP) stdio adapter:

```sh
imp acp
```

ACP is a JSON-RPC protocol for editor/agent integration. It is separate from imp's `--mode rpc` JSONL host protocol; the two are not wire-compatible.

## Current status

The adapter is suitable for protocol and session-lifecycle smoke testing, not daily editor use.

Implemented:

- `initialize` for ACP protocol version 1;
- newline-delimited JSON-RPC over stdio;
- conservative capability advertisement;
- `session/new` with absolute `cwd` validation;
- durable imp session creation;
- `session/load` and `session/resume` by imp session id;
- history replay through `session/update` during load;
- prompt block parsing and durable user-message persistence;
- `session/cancel` notification state for the scaffold;
- imp-message to ACP update mapping helpers.

Not implemented:

- live agent/model execution from `session/prompt`;
- real assistant streaming;
- permission requests for tool and UI approvals;
- full policy-denial UX;
- client-supplied MCP servers;
- image or audio prompt content;
- ACP registry metadata.

A prompt currently returns scaffold metadata and may emit a short acknowledgement. It does not contact a provider or execute tools.

## Transport

- stdin: one JSON-RPC 2.0 message per line;
- stdout: one JSON-RPC 2.0 message per line;
- stdout must contain only ACP messages;
- diagnostics belong on stderr.

## Smoke test

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"smoke"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"'"$PWD"'","mcpServers":[]}}' \
  | imp acp
```

Expected:

- response 1 contains `result.protocolVersion: 1` and `agentInfo.name: "imp"`;
- response 2 contains a durable imp `sessionId`.

## Editor configuration

```json
{
  "command": "imp",
  "args": ["acp"]
}
```

Normal global model/provider options can appear before the subcommand:

```json
{
  "command": "imp",
  "args": ["--provider", "anthropic", "--model", "claude-sonnet-4", "acp"]
}
```

Those options will matter once live turn execution is wired.

## Troubleshooting

- Invalid JSON: ensure no logs are written to stdout in ACP mode.
- `session/new` or load failure: send an absolute `cwd`.
- Unknown session: use an id from imp's durable session store.
- MCP configuration failure: send an empty `mcpServers` list.
- Stubbed prompt response: expected; live agent turns are not connected.
