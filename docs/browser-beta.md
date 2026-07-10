# Browser beta

imp includes an opt-in semantic browser powered by Lightpanda. It is suitable for stateful JavaScript navigation, structured page observation, extraction, and policy-controlled page interaction.

This feature is a production beta. Lightpanda is itself beta software, and site compatibility is not equivalent to Firefox or Chromium.

## Scope

The browser supports:

- isolated, stateful Lightpanda sessions;
- HTTP and HTTPS navigation;
- semantic interactive-element observations;
- markdown, links, forms, structured data, and schema extraction;
- click, fill, key, select, check, scroll, and wait actions;
- console and current-URL inspection;
- scoped interactive approvals;
- TUI cards, settings, diagnostics, RPC events, traces, and durable sanitized lifecycle records.

The browser does not support:

- screenshots or graphical page rendering;
- coordinate-based interaction;
- canvas, WebGL, or visual layout verification;
- a visible browser window or human takeover;
- arbitrary desktop application control.

Do not describe the Lightpanda integration as visual computer use. It is a semantic browser backend.

## Requirements and support

imp requires Lightpanda 0.3.4 or newer and validates the MCP tool surface at startup.

Current installation support:

- macOS: Homebrew installation is available from the CLI and TUI;
- Linux: install Lightpanda manually and ensure its executable is on `PATH`, or configure `browser.binary`;
- Windows: no native managed installation is provided.

Lightpanda may fail on sites that depend on unimplemented Web APIs, browser-specific behavior, advanced graphics, anti-automation controls, or unsupported authentication flows.

## Install and diagnose

On macOS:

```sh
imp browser install --yes
```

The command shows the package-manager command and requires explicit confirmation. imp does not download unsigned release binaries.

Check readiness:

```sh
imp browser doctor
imp browser doctor --json
```

The doctor validates resolved configuration, executable permissions, Lightpanda version, MCP initialization, and required tools. The TUI **Browser** settings tab uses the same API.

## Configuration

```toml
[browser]
enabled = true
# binary = "/path/to/lightpanda"
max_sessions = 2
timeout_ms = 30000
idle_timeout_seconds = 300
max_response_bytes = 1048576
obey_robots = true
block_private_networks = true

[policy]
browser_input = "ask"
```

`block_private_networks = true` is the production default. Disable it only when access to trusted private targets is explicitly required.

## Security model

Browser sessions run as isolated Lightpanda subprocesses with ephemeral state. imp clears the child environment and disables Lightpanda telemetry and core dumps.

Browser input defaults to `ask`:

- interactive sessions can approve once, for the current domain and session, or for the session;
- headless sessions fail closed unless policy explicitly allows input;
- sensitive actions require fresh approval;
- domain leases do not transfer to a different domain;
- leases are memory-only and end with the session or imp process.

Filled values are redacted from approval prompts, tool events, traces, durable sessions, RPC lifecycle events, and TUI summaries.

Navigation accepts only HTTP and HTTPS URLs. It rejects credentials in URLs, invalid hosts, `file:`, `javascript:`, and other schemes.

## Operations

Sessions have bounded operation timeouts, response and output sizes, idle lifetime, and action sequence numbers. Protocol failure, timeout, cancellation, or process exit invalidates the affected session. imp does not automatically retry ambiguous browser input.

Structured events are emitted as `browser_event` in JSONL RPC and `browser_updated` in the canonical runtime stream. Sanitized records also appear in sessions, traces, and run evidence. Events do not contain page bodies or filled values.

## Troubleshooting

### Lightpanda was not found

Run `imp browser doctor`, then install Lightpanda or set an absolute `browser.binary` path.

### Version or MCP tools are incompatible

Upgrade Lightpanda and rerun the doctor. imp rejects startup if required tools are missing rather than silently degrading behavior.

### A site does not work

Confirm that it does not require screenshots, canvas interaction, unsupported browser APIs, or anti-bot circumvention. Reports should include doctor JSON, imp and Lightpanda versions, the URL domain, and a sanitized error. Never include credentials, form values, cookies, or private page content.

### Browser input is denied

Interactive use requires approval when `browser_input = "ask"`. Noninteractive use must explicitly allow input in a trusted environment. Do not use blanket `allow` merely to bypass a sensitive-action prompt.

### Private or loopback URL is blocked

This is expected with the production default. Keep private-network blocking enabled unless the target and execution context are explicitly trusted.

## Verification and compatibility

The normal suite includes offline protocol fault injection. A deterministic real-Lightpanda fixture is available:

```sh
LIGHTPANDA_BIN=/verified/path/lightpanda \
  cargo test -p imp-core real_lightpanda_deterministic_fixture -- --ignored
```

CI or release automation should supply a pinned, checksum-controlled artifact. The test does not download a browser binary.

## Remaining beta work

The production-beta contract is complete. Remaining work is post-beta hardening and expansion:

- run the real-Lightpanda fixture in CI with a verified artifact;
- publish measured compatibility results across supported releases;
- add managed Linux installation when a verifiable package source is available;
- add explicit TUI stop and approval-revocation controls;
- evaluate a rendered Chromium backend for screenshots and visual interaction.
