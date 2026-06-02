# Proposal: Benchmark observability, token accounting, and fast-path diagnostics

## Status

Proposal / feature request.

## Background

During the Goose / Imp / Codex / Pi benchmark work in `/Users/asher/agent-bench`, Imp performed well on objective correctness and was often fastest by wall time, especially on medium and harder repository tasks. The benchmark also exposed two product gaps:

1. Imp is hard to audit when a run is unexpectedly slow or token-heavy.
2. Imp's current token reporting is not directly comparable to Codex's human `tokens used` display.

Codex's human output reports a cache-adjusted/blended token total:

```text
codex displayed tokens = input_tokens - cached_input_tokens + output_tokens
```

Imp currently prints raw accumulated input/output totals:

```text
[tokens: ↑42463 ↓975 | cost: $0.0000]
```

Imp's OpenAI provider already parses cached input into `Usage.cache_read_tokens`, but common CLI/JSON surfaces do not expose cache read/write, raw totals, or effective totals. That made Imp initially look much more token-expensive than it likely is.

Structured reruns showed the distinction matters. On representative benchmark tasks, Codex raw provider totals were often much larger than Imp's raw totals, while Codex's cache-adjusted totals were much lower because most Codex input was cached.


## Benchmark evidence

Representative benchmark data from `/Users/asher/agent-bench` shows why this proposal matters.

### Structured token subset

The following subset used structured Codex JSON output and Imp JSON output where available.

| Agent | Runs | Pass rate | Mean task median wall ms | Mean task median raw total | Mean task median effective/blended total | Cache coverage |
|---|---:|---:|---:|---:|---:|---:|
| `codex_json` | 12 | 100% | 58,095 | 161,171 | 16,782 | 12/12 |
| `imp_minimal` | 12 | 100% | 51,952 | 56,922 | 56,922 upper bound | 0/12 |

Interpretation:

- Codex raw totals can be much larger than Imp raw totals.
- Codex exposes cached input, so its effective/blended total can be computed.
- Imp's JSON output did not expose cache-read/cache-write, so Imp's effective total is unknown and currently appears as the raw total.

### Hard task smoke data

| Task | Agent | Passed | Wall ms | Raw total tokens | Cached input | Effective/blended tokens | Notes |
|---|---|---:|---:|---:|---:|---:|---|
| `011-larger-repo-navigation` | `imp_minimal` | true | 43,287 | 66,918 | n/a | 66,918 upper bound | Minimal one-file fix. |
| `011-larger-repo-navigation` | `codex_json` | true | 51,674 | 180,857 | 155,648 | 25,209 | Heavy cache use. |
| `014-semver-ranges` | `imp` | true | 161,659 | 228,349 | n/a | n/a | Smaller diff than Codex. |
| `014-semver-ranges` | `codex_json` | true | 162,589 | 238,533 | 199,552 | 38,981 | Similar wall time, cache-adjusted tokens much lower. |

### Imp speed variance examples

The same task can vary substantially depending on tool/error/recovery turns.

| Task/run family | Fast run wall ms | Slow run wall ms | Observed correlation |
|---|---:|---:|---|
| `001-small-bugfix` with `imp_minimal` | 27,383 | 49,155 | Slower runs had more bash calls, more errors, and more raw tokens. |
| `007-multifile-debug` with `imp_minimal` | 58,482 | 76,048 | Slower runs had more bash calls, errors, and tokens. |
| `012-state-machine-migration` with regular `imp` | 105,370 | n/a | More regular-system context and larger diff than Codex. |
| `014-semver-ranges` with regular `imp` | 161,659 | n/a | Hard synthesis dominated wall time despite similar raw tokens to task 013. |

This suggests the most useful next product work is not just optimizing the model path; it is exposing why the loop continued, how much each turn cost, and which tool calls created extra obligations.

## Relevant implementation locations

These files were inspected during the audit and are likely implementation touchpoints:

| Area | File |
|---|---|
| Final CLI usage output | `crates/imp-cli/src/lib.rs` |
| Usage type and cost calculation | `crates/imp-llm/src/usage.rs` |
| OpenAI cached token parsing | `crates/imp-llm/src/providers/openai.rs` |
| OpenAI Codex request shaping | `crates/imp-llm/src/providers/openai_codex.rs` |
| Agent turn loop and accumulated usage | `crates/imp-core/src/agent/run_loop.rs` |
| Stop/continue policy | `crates/imp-core/src/agent/loop_policy.rs` |
| Post-turn assessment and recovery nudges | `crates/imp-core/src/agent/mod.rs` |
| System prompt and resource assembly | `crates/imp-core/src/builder.rs`, `crates/imp-core/src/system_prompt.rs` |
| Runtime event types | `crates/imp-core/src/agent/events.rs`, `crates/imp-core/src/runtime.rs` |

## Goals

- Make Imp's token/cost reporting cache-aware and comparable to Codex-style accounting.
- Make slow/fast benchmark behavior explainable per run and per turn.
- Preserve Imp's correctness-oriented loop behavior while identifying avoidable extra turns, noisy tool output, and failed tool calls.
- Provide a first-class fresh/benchmark mode that disables user/project context without disabling normal tools.
- Improve small-task latency and reduce long-tail variance.

## Non-goals

- Do not remove normal durable workflow behavior from regular Imp modes.
- Do not hide raw token usage; both raw and effective/cache-adjusted totals should be visible.
- Do not weaken safety/verification behavior to win benchmarks.
- Do not require external benchmark harnesses to parse human prose.

## Requested features

### 1. Expose cache-aware token usage in CLI and JSON output

#### Problem

Imp one-shot output currently exposes input/output but not cache read/write. This makes raw totals look worse than Codex's cache-adjusted human display.

#### Request

Expose full cache-aware usage in all non-interactive outputs.

Suggested JSON shape:

```json
{
  "usage": {
    "input_tokens": 42463,
    "cache_read_tokens": 30000,
    "cache_write_tokens": 0,
    "output_tokens": 975,
    "raw_total_tokens": 43438,
    "effective_total_tokens": 13438
  },
  "cost": {
    "input": 0.0,
    "cache_read": 0.0,
    "cache_write": 0.0,
    "output": 0.0,
    "total": 0.0
  }
}
```

Definitions:

```text
raw_total_tokens = input_tokens + output_tokens
effective_total_tokens = max(input_tokens - cache_read_tokens, 0) + output_tokens
```

#### Acceptance criteria

- `imp -p ... --output json` includes `cache_read_tokens`, `cache_write_tokens`, `raw_total_tokens`, and `effective_total_tokens`.
- Human output clearly distinguishes raw from effective/cache-adjusted tokens.
- Existing `usage.input_tokens` and `usage.output_tokens` remain backward-compatible.
- Unit tests cover cache-read, cache-write, and no-cache cases.

---

### 2. Emit per-turn LLM usage telemetry

#### Problem

Final accumulated usage does not explain why one run was fast and another was slow. In the benchmark, Imp was fastest when it reached `edit + verification = work completed` quickly, but slower when extra interpretation/recovery turns were added.

#### Request

Emit structured per-turn/per-request LLM telemetry in JSON/runtime event mode.

Suggested event:

```json
{
  "type": "llm_request_completed",
  "turn": 3,
  "duration_ms": 1850,
  "message_count": 9,
  "tool_definition_count": 12,
  "request_bytes": 58231,
  "usage": {
    "input_tokens": 12000,
    "cache_read_tokens": 9000,
    "cache_write_tokens": 0,
    "output_tokens": 600,
    "raw_total_tokens": 12600,
    "effective_total_tokens": 3600
  }
}
```

#### Acceptance criteria

- Runtime JSON emits one event per provider request.
- Event includes request duration and full token breakdown.
- Event includes message count, tool count, and request byte count.
- Tests cover aggregation into final usage.

---

### 3. Add explicit fresh benchmark mode

#### Problem

Reproducible benchmarks need to disable local/user context while keeping normal tools. Today this requires knowing that `--system-prompt` skips assembled AGENTS/skills context, while tools are still sent separately.

#### Request

Add a first-class fresh/benchmark mode.

Example:

```bash
imp -p "..." \
  --fresh \
  --system-prompt "You are a coding agent..." \
  --all-tools \
  --model gpt-5.5 \
  --thinking high \
  --output json
```

Suggested behavior for `--fresh`:

- no session persistence;
- no `AGENTS.md` / `CLAUDE.md` discovery unless explicitly enabled;
- no skills discovery unless explicitly enabled;
- no user facts, memory, personality, or soul;
- no project memory;
- deterministic minimal config surface;
- tools remain enabled unless `--no-tools` is provided.

#### Acceptance criteria

- `imp --fresh -p "say ok" --output json` does not load user/project markdown context, skills, memory, or sessions.
- `--fresh` can be combined with normal/default tools.
- CLI help documents exactly what is disabled.
- Tests verify `AGENTS.md` and skills are not included under `--fresh`.

---

### 4. Add token-overhead diagnostics for system prompt and tool schemas

#### Problem

Even with a tiny system prompt and no session, `imp -p "say ok"` reports non-trivial input tokens with tools enabled. That is probably mostly tool schema overhead, but there is no easy way to verify.

#### Request

Add diagnostics that estimate or report the byte/token contribution of major request sections.

Example under `--usage-debug`:

```json
{
  "request_breakdown": {
    "system_prompt_bytes": 97,
    "tool_schema_bytes": 18342,
    "message_history_bytes": 19,
    "tool_result_bytes": 0,
    "estimated_system_prompt_tokens": 24,
    "estimated_tool_schema_tokens": 4300,
    "estimated_message_history_tokens": 5
  }
}
```

#### Acceptance criteria

- Works in non-interactive JSON output.
- Reports byte counts at minimum; token estimates are acceptable if exact tokenization is unavailable.
- Does not include secret values in diagnostics.
- Makes trivial-prompt overhead explainable.

---

### 5. Improve default file discovery behavior

#### Problem

Benchmark transcripts showed Imp sometimes using noisy discovery commands such as:

```bash
find . -maxdepth 3 -type f
```

which listed `.git` internals. Codex tended to use `rg --files`, which avoided VCS internals and produced cleaner context.

#### Request

Improve tool guidance and/or tool behavior so Imp avoids noisy file discovery by default.

Options:

1. Prompt/tool guidance: prefer `rg --files` over `find .` for repository file listing.
2. Shell output filtering: discourage or truncate `.git`, `node_modules`, `__pycache__`, and build artifacts.
3. Add a native file-list tool that ignores VCS/build directories by default.

#### Acceptance criteria

- On small repo tasks, Imp should not list `.git` internals during initial discovery.
- Benchmark telemetry shows reduced file-discovery output bytes.
- Behavior remains flexible: agents can still inspect ignored directories explicitly if needed.

---

### 6. Reduce failed edit/tool-call rate

#### Problem

Benchmark transcripts showed malformed edit calls, for example:

```text
[tool: edit src/text_utils.py]
[error: Missing or empty edits array]
```

This wastes tool time, model turns, and context tokens.

#### Request

Improve model-facing edit tool ergonomics and failure recovery.

Possible changes:

- clearer schema descriptions;
- examples in tool description;
- validation errors that include the minimal valid shape;
- optional simpler edit API for common exact replacements;
- telemetry for failed tool calls.

#### Acceptance criteria

- Edit validation errors include a concise valid example.
- Benchmark output records failed tool calls per run.
- Failed edit call rate decreases on repeated small bugfix tasks.

---

### 7. Add benchmark-friendly structured event export

#### Problem

External harnesses currently scrape human transcripts. That is brittle and causes apples-to-oranges comparisons.

#### Request

Provide a stable benchmark/event output mode.

Example:

```bash
imp -p "..." --output benchmark-json
```

Final event should include:

```json
{
  "status": "done",
  "wall_time_ms": 42123,
  "ttft_ms": 2810,
  "turns": 4,
  "tool_calls": 8,
  "failed_tool_calls": 1,
  "files_read": 3,
  "files_written": 1,
  "commands_run": 3,
  "usage": {
    "input_tokens": 42463,
    "cache_read_tokens": 30000,
    "output_tokens": 975,
    "effective_total_tokens": 13438
  },
  "verification": {
    "commands": ["python3 -m unittest discover -s tests -p 'test_*.py'"],
    "passed": true
  }
}
```

#### Acceptance criteria

- Stable JSON schema for benchmark output.
- Captures timing, tool, file, command, and token metrics.
- Does not require parsing human output.
- Documented in CLI help or docs.

---

### 8. Preserve streaming telemetry in JSON output

#### Problem

When using final JSON output, TTFT can appear equal to wall time because the harness sees only the final JSON object. Human text mode streams earlier but lacks structured usage.

#### Request

Add JSONL streaming events plus a final summary object.

Example:

```bash
imp -p "..." --output jsonl
```

Events should include:

- first model stream event;
- first text delta;
- tool call started/completed;
- LLM request completed;
- final summary.

#### Acceptance criteria

- JSONL output preserves meaningful TTFT measurement.
- Final summary remains easy to parse.
- Human output remains unchanged unless explicitly requested.

---

### 9. Add small-task fast path / lower ceremony mode

#### Problem

Imp is competitive overall and often fastest on medium/harder tasks, but fixed overhead can dominate tiny one-file bugfixes.

#### Request

Add or tune a low-ceremony mode for simple tasks.

Possible approaches:

- fewer initial checks for tiny repos;
- avoid broad discovery if prompt/test names point to a small target;
- faster first tool selection;
- context/tool schema caching;
- adaptive closeout when verification passes.

#### Acceptance criteria

- On a small one-file bugfix benchmark, median wall time improves without reducing pass rate.
- Tool calls and failed tool calls do not increase.
- Verification behavior remains intact.

---

### 10. Provider parity audit for OpenAI Codex route

#### Problem

Imp, Codex, Goose, and Pi may all route “GPT-5.5 high” through subtly different providers and request shapes. Cost and speed comparisons need provider/request parity.

#### Request

Add a debug mode that prints sanitized provider request metadata:

```json
{
  "provider": "openai-codex",
  "model": "gpt-5.5",
  "reasoning_effort": "high",
  "stream": true,
  "store": false,
  "parallel_tool_calls": true,
  "tool_count": 12,
  "cache_key_present": true,
  "unsupported_fields_stripped": ["temperature", "max_output_tokens"]
}
```

#### Acceptance criteria

- Metadata can be emitted without secrets or prompt contents.
- Helps compare Imp request shape to Codex CLI request shape.
- Tests cover OpenAI Codex provider request field handling.

## Runtime behavior findings to validate

Benchmark timing variance appears to depend on several runtime paths:

- More tool/model turns lead to much longer wall time.
- Failed bash commands can create recovery obligations and follow-up turns.
- Work-completed detection makes Imp fast when it recognizes `edit + successful verification` evidence.
- Noisy discovery and malformed edit calls add context and sometimes extra turns.
- JSON final-output mode can hide true TTFT by buffering until completion.

These should be validated with the telemetry features above rather than inferred from transcripts.

## Suggested implementation order

1. Expose cache-aware final usage in JSON and human output.
2. Add structured per-turn usage/timing events.
3. Add benchmark JSONL output mode.
4. Add `--fresh` mode.
5. Add request-overhead diagnostics.
6. Improve file discovery guidance/tooling.
7. Tune small-task fast path using benchmark evidence.

## Related benchmark artifacts

- Benchmark repo: `/Users/asher/agent-bench`
- Feature request draft copied from benchmark notes: `/Users/asher/agent-bench/imp-feature-requests.md`
- Token summary: `/Users/asher/agent-bench/results/summaries/token-cost-comparison.md`
- Larger/harder task smoke summaries live under `/Users/asher/agent-bench/results/summaries/`
