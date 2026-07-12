# Process runtime

## Status

Imp ships one typed process runtime in `imp-core`. It owns one-shot shell commands,
shell hooks, and managed Bash jobs. It does not modify the shared `RuntimeEvent` model in
this phase.

Pipe mode is shipped. `ProcessMode::Pty` is retained in the domain model but the host
backend returns `UnsupportedCapability("pty")`; it never silently substitutes pipes.
No zmx or tmux dependency is used. Those tools solve user-terminal persistence, while
Imp must retain grant ownership, bounded redacted output, cleanup, and run attribution.

## Ownership and lifecycle

`ProcessManager` owns process records, child supervision, stdin handles, output drains,
timeouts, and process-local observations. Tools never receive a raw
`tokio::process::Child`.

Each launch creates an opaque UUID-backed `ProcessId`. This ID is stable within the
manager and is distinct from the operating-system PID. OS PIDs are private lifecycle
data used only for process-group control.

On Unix, every command starts in a new process group. Stdout and stderr are drained by
independent tasks so a slow consumer cannot block the child. The supervisor waits for
the child, joins both drains before publishing terminal completion, and therefore
reaps the child instead of leaving a zombie.

Terminal records and unread retained output remain inspectable. The manager retains at
most 256 records and prunes the oldest terminal records before accepting additional
processes. Active records are never pruned. Repeated `wait` and `stop` return retained
terminal metadata. Unknown IDs and writes to exited processes are typed errors.

Dropping the final manager owner sends SIGKILL to every active Unix process group and
notifies supervisors to reap their children. Session shutdown therefore does not leave
manager-owned jobs running.

## Output and cursors

Each request chooses an output retention byte limit. Zero selects the safe default of
64 KiB; requests are capped at 4 MiB. Output is stored in a bounded ring organized as
stdout/stderr chunks.

`OutputCursor` contains the process ID and a monotonic byte position. A read returns:

- chunks after the requested position, bounded to 64 KiB per response;
- stdout or stderr origin for every chunk;
- the next cursor;
- current process state;
- whether unread retained bytes were evicted;
- whether more retained bytes remain for another response.

Cursor/process mismatches fail rather than reading unrelated output. Bytes are decoded
with `String::from_utf8_lossy`, so invalid UTF-8 is explicit and non-fatal. Reads never
clone the complete retained history.

Injected secret values are redacted before bytes enter retention. The streaming
redactor holds enough suffix bytes to detect a secret split across OS read boundaries.
Secret values are also omitted from request debug output, process listings, errors,
and events.

`ProcessEvent` is a bounded broadcast observation API:

- `Started`
- `OutputAvailable`
- `StateChanged`
- `Exited`

Events contain IDs, state, exits, and cursors, never unbounded output. Lagging observers
must recover by reading from their cursor and checking eviction.

## Execution grants

Every `ProcessRequest` requires an `ExecutionGrant`. Bash constructs it only after the
normal tool mode, run-policy, reference-monitor, cwd/workdir, and secret-policy checks
have authorized the tool call. The frozen grant records:

- readable and writable roots;
- network permission;
- allowed environment names;
- approved secret IDs and environment names;
- child-process restrictions;
- isolation requirement.

The stages remain separate:

1. Imp policy evaluates the tool call.
2. The caller constructs a frozen grant.
3. A process backend validates and enforces what it can.
4. `EnforcementSummary` reports requested, preflight-validated, and enforced controls.

The shipped host backend enforces environment reconstruction, approved secret
injection, and Unix process groups. It preflight-validates the working directory. It
does **not** enforce filesystem roots, network isolation, or child-process denial at
the OS boundary, and never reports those controls as enforced.

If `IsolationRequirement::Required` is requested, the host backend fails before spawn.
There is no downgrade. Future backends may implement macOS Seatbelt, Linux
bubblewrap/seccomp, or remote execution behind `ProcessBackend`; no fake backend is
present.

## Cancellation and cleanup

A request timeout kills the entire process group and classifies the result as timed
out. Cancellation kills the process group and classifies it as cancelled. `stop` sends
SIGTERM, waits for its bounded grace period, then sends SIGKILL if necessary. Output is
drained before terminal state is visible.

The manager does not hold blocking mutex guards across `.await`. Control messages and
notifications separate state mutation from asynchronous effects. Non-interactive callers
can explicitly close stdin through the manager so child reads observe EOF without
inheriting the host terminal.

## Bash

Normal Bash calls preserve the existing one-shot contract: configured shell, workdir,
hard timeout, cancellation, streamed updates, sanitization, secret injection and
redaction, search head truncation, ordinary-command tail truncation, byte/line limits,
failure hints, no-match exits, details fields, and optional Rush behavior. The normal
shell backend always uses `ProcessManager`.

The same Bash tool supports explicit managed jobs without increasing Imp's tool count:

```json
{ "command": "npm run dev", "background": true, "yield-time_ms": 500 }
```

A background call bypasses Rush, starts through the normal host process backend,
observes startup for at most five seconds, and returns a non-OS `job_id`, bounded output,
state, exit data, eviction, and truncation. It never auto-detaches a slow foreground
command.

The same tool manages that job:

```json
{ "job_id": "...", "yield_time_ms": 500 }
{ "job_id": "...", "stdin": "reload\n", "yield_time_ms": 500 }
{ "job_id": "...", "stop": true }
```

Bash stores cursors internally and returns only newly observed output. Polling is
event-driven. `stop` performs graceful process-tree cleanup and is idempotent. The two
request shapes are mutually exclusive; requests containing both `command` and `job_id`
fail. PTY-dependent interactive programs remain unsupported.

## Shell hooks

Blocking and non-blocking shell hooks use the same one-shot manager path. Existing
ordering, callbacks, timeout, stdout/stderr capture, process-tree cleanup, and
background failure reporting remain unchanged. The hook protocol was not redesigned.

TOML-defined shell tools also use a shared `ProcessManager`. They preserve parameter
interpolation, install hints, stdout-before-stderr composition, truncation, timeouts,
and result details while adding in-flight cancellation and explicit stdin EOF.

Workflow verification command gates use their runner's shared manager. Gate state
transitions, private artifacts, separate stream summaries, byte accounting, truncation,
and timeout blocking remain owned by the verification layer.

Guardrail check batches use one manager while preserving sequential execution,
stdout-then-stderr reporting, context truncation, timeout errors, and advisory or
enforcing result semantics.

Workflow command and changed-files checks use one manager per step run. They preserve
zero-test rejection, exit metadata, ordered workflow event updates, and status
reconciliation while adding bounded output, stdin EOF, five-minute timeouts, and
process-tree cleanup.

Browser diagnostics and persistent MCP sessions use the process runtime. The MCP paths
preserve environment isolation, protocol arguments, incremental line framing, request
writes, response limits and parsing, timeout cleanup, required-tool reporting, and
graceful session shutdown.

## Remaining subprocess inventory

The following production paths still launch processes directly and are not migrated in
this phase:

- `crates/imp-lua/src/bridge.rs` — Lua tool subprocess execution;
- `crates/imp-core/src/typescript_extensions/bun_runner.rs` — TypeScript/Bun hosts;
- `crates/imp-core/src/tools/prototype.rs` — prototype execution and runtime probes;
- `crates/imp-core/src/workflow/worktree_run.rs` — workflow Git children;
- `crates/imp-core/src/tools/git.rs` — Git subprocesses;
- `crates/imp-core/src/repo_intelligence.rs` and
  `crates/imp-core/src/tools/scan/mod.rs` — read-only Git discovery;
- `crates/imp-cli/src/lib.rs` — installer/update, browser-open, and CLI helpers.

Test-only runtime probes and fixture setup are excluded from the migration inventory.
Future migrations should use the manager in the owning layer rather than introducing
adapter-specific process semantics.

## RuntimeEvent integration follow-up

This branch intentionally does not edit `RuntimeEventKind`, `crates/imp-core/src/runtime`,
agent event schemas, TUI, ACP, or GUI projections.

After the authoritative RuntimeEvent/RuntimeState branch lands, add one runtime-owned
adapter that subscribes to `ProcessManager::subscribe()` and maps bounded
`ProcessEvent` values into the authoritative event model. The adapter must preserve the
process ID, cursor, state, exit classification, timestamps, and enforcement summary.
It must not embed output bytes; consumers read output through the manager using the
published cursor. Add persistence/replay and projection tests in that integration
branch before declaring process events supported on user-facing surfaces.
