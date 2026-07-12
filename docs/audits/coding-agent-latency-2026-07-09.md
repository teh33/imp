# Coding-agent latency audit

Date: 2026-07-09
Model: `openai-codex/gpt-5.6-sol`
Thinking: `xhigh`

## Conclusion

imp's local Rust runtime and provider transport are not the source of the observed slowdown against vanilla Pi.

On representative local coding tasks, 97–98% of imp wall time is spent waiting for provider responses. imp is slower because it induces more provider rounds: task planning and step updates, broader discovery, edit recovery, verification recovery, and final closeout each trigger another model request.

The first optimization target should be a small-task path that keeps runtime evidence and verification enforcement but does not require model-authored task plans and per-step updates.

## Paired baseline

Three tasks ran three times per agent with identical prompts, verifier constraints, provider, model, and thinking level.

| Task | imp | Pi | imp median | Pi median | imp raw | Pi raw | imp effective | Pi effective |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `one-file-fix` | 3/3 | 3/3 | 55.5s | 20.4s | 205,811 | 71,227 | 66,035 | 26,683 |
| `no-op` | 3/3 | 3/3 | 25.9s | 13.1s | 117,355 | 52,997 | 32,875 | 17,157 |
| `preserve-dirty-files` | 3/3 | 3/3 | 82.4s | 21.4s | 280,512 | 86,671 | 82,880 | 17,551 |
| **Overall** | **9/9** | **9/9** | **55.5s** | **20.4s** | **603,678** | **210,895** | **181,790** | **61,391** |

`raw` is uncached input + cache reads + cache writes + output. `effective` is uncached input + cache writes + output. Pi reports uncached input and cache reads separately; imp preserves OpenAI's shape where cached tokens are included in input. The original paired aggregator compared imp effective tokens against Pi raw tokens and produced an invalid claim that imp used fewer tokens.

After normalization, the original matrix shows imp processed 2.86× as many raw tokens and 2.96× as many effective tokens. It also used 2.85× as many output tokens: 13,857 versus 4,858. Those figures align with imp's additional provider rounds and higher latency.

Reports:

- `1783629350337-imp-vs-pi-cc4f201560f049308fbc23f902c6fff2.json`
- `1783629572238-imp-vs-pi-8e185458a70a4db2aed1f0c9fdcb3d6e.json`
- `1783629693961-imp-vs-pi-4f68408a48ae49dbbe22316b13ebbd5d.json`

## Instrumented imp runs

The print-mode outcome now records provider requests, provider time, tool time, context assembly time, and post-turn assessment time.

| Task | Wall | Provider | Provider share | Context | Tools | Requests |
|---|---:|---:|---:|---:|---:|---:|
| `no-op` | 29.922s | 28.996s | 96.9% | 623ms | 218ms | 8 |
| `one-file-fix` | 47.546s | 46.549s | 97.9% | 691ms | 249ms | 11 |

Post-turn assessment rounded to 0ms in both runs. Tool execution was below 1% of wall time. Context assembly was below 2.1%.

## Provider transport control

A no-tool prompt, `Reply with exactly OK.`, was run three times through both agents with extensions, skills, prompt templates, context files, and tools disabled for Pi.

imp was roughly 2.3 times faster by median wall time. This rules out imp's OpenAI transport as the cause of the coding-task slowdown.

## Task-ledger isolation

The full no-op run used eight requests. Its tool sequence alternated ordinary work with four `task` calls:

```text
scan, task, git, task, read, task, bash, task
```

With a broad native tool surface but only `task` removed, the same no-op task completed in:

- 14.075s wall time;
- 13.359s provider time;
- four requests;
- five tool calls;
- zero failed calls;
- 8,060 effective tokens.

With only `read,bash,edit,write`, task state disabled, it completed in:

- 19.339s wall time;
- 18.603s provider time;
- four requests;
- five tool calls;
- zero failed calls;
- 6,983 effective tokens.

The broader tool surface did not cause the extra rounds. The model-managed task plan and step closeout did.

For `one-file-fix`, the reduced four-tool surface completed in 26.684s with seven requests. Two failed calls remained: an edit retry and a command recovery. These explain much of the remaining gap from Pi's five-request path.

## Bugs found during the audit

### Tool accounting

Print mode used an `active_tool` slot to name completed calls. Parallel or adjacent task calls could clear that slot before completion, producing successful `unknown` records and inflating `failed_tool_calls`.

Tool completion now uses `result.tool_name`, which is authoritative.

### `--tools` was inert in headless sessions

The CLI parsed `--tools`, but it was not propagated into `SessionOptions` or applied to the agent registry. Earlier reduced-surface experiments therefore still exposed the full native surface.

`SessionOptions.enabled_tools` now applies a canonical registry allowlist. Removing `task` also disables task-state projection and closeout enforcement.

## Implemented task activation fix

The task system now has separate evidence and planning layers:

- runtime evidence remains enabled for ordinary coding work;
- `task` is omitted from the request tool schema for straightforward one-shot prompts;
- ordinary prompts do not receive a task projection until runtime evidence exists;
- explicit planning language activates planning immediately;
- unfinished work continuing across a user prompt activates planning;
- three changed files or two unresolved command failures activate planning dynamically;
- once planning activates, the request receives the `task` tool and a projection explaining its purpose;
- verification debt and closeout enforcement remain active even while planning is dormant.

A post-change default-tool no-op run passed with no task calls and five provider requests, down from the prior eight-request instrumented run. A three-repeat paired regression then passed 3/3 for both imp and Pi: imp's median fell from 25.9s to 17.7s versus Pi's 13.9s. Normalized usage was imp 76,815 raw / 23,567 effective tokens versus Pi 56,878 raw / 21,038 effective tokens. imp therefore used 35% more raw tokens, 12% more effective tokens, and 67% more output tokens while remaining 21% slower by aggregate wall time, or 27% slower by median run time. The apparent throughput is not 4,000 generated tokens per second: cached and prompt tokens are processed input, not generated output. Across the three runs, imp generated about 26 output tokens per wall-clock second and Pi about 19; wall time is dominated by per-request latency, reasoning, and 14 versus 13 sequential provider rounds rather than output streaming throughput. A default-tool one-file repair also passed with no task calls; its remaining eight requests included one failed edit and one failed command, confirming the next latency target is recovery reliability rather than task bookkeeping.

Paired regression report:

- `1783634003355-imp-vs-pi-e804bbaa42fc47afae3f6852bb0d65b0.json`

## Corrected external-agent smoke matrix

The runner was extended to compare imp, vanilla Pi, Codex CLI, and vanilla OpenCode in one report. Codex ran with an isolated `CODEX_HOME`; OpenCode ran with `--pure`, isolated config, and external skills disabled. The active oh-my-openagent installation was therefore not part of the OpenCode baseline.

The first four-agent run is invalid. Eval worktrees lived under the imp repository and child `PWD` was inherited from the parent checkout. imp and OpenCode discovered the source repository and modified source fixtures rather than their isolated copies. The runner now:

- defaults worktrees to the OS temporary directory outside the source repository;
- canonicalizes child cwd and `PWD`;
- gives OpenCode an explicit `--dir`;
- namespaces worktrees by comparison id and agent;
- snapshots local fixture sources, fails any escaping agent, preserves the violating tree, and restores the source.

A malicious fake-agent probe verified the boundary end to end. The corrected one-pass smoke matrix then passed all three verifiers for all four agents:

| Agent | Passes | Aggregate wall | Raw tokens | Effective tokens | Output tokens | Rounds | Tool calls |
|---|---:|---:|---:|---:|---:|---:|---:|
| Pi | 3/3 | 62.9s | 58,665 | 24,873 | 1,391 | 16 | 21 |
| imp | 3/3 | 96.1s | 108,956 | 40,348 | 2,500 | 20 | 27 |
| Codex | 3/3 | 102.5s | 234,527 | 33,055 | 3,826 | 3 top-level turns | 14 |
| OpenCode | 3/3 | 138.2s | 146,108 | 29,372 | 4,048 | 19 | 30 |

This is one run per task, not a stable ranking. It is diagnostic evidence. Relative to Pi in this sample, imp was 53% slower, used 86% more raw tokens, 62% more effective tokens, 80% more output tokens, 25% more rounds, and 29% more tool calls. Codex's raw count was dominated by cache reads. OpenCode used fewer effective tokens than imp but had the highest wall time.

Corrected reports:

- `1783638205250-imp-vs-pi-vs-codex-vs-opencode-bba66de9c9f141f9a8ddaead8a87b6cf.json`
- `1783638302020-imp-vs-pi-vs-codex-vs-opencode-b68f64f44c7f4bebba3a8189bda60b86.json`
- `1783638449994-imp-vs-pi-vs-codex-vs-opencode-3546f84c2932472bb0b5865ebb963fde.json`

## First-attempt tool reliability

The corrected traces exposed deterministic recovery failures in imp's `edit` dispatch. GPT-5.6 Sol populated optional schema fields with defaults while also sending a valid exact edit:

- `edits: []` incorrectly activated transaction mode;
- `anchor_start: ""` incorrectly activated anchored mode.

The dispatcher now activates transaction or anchored mode only for non-empty values, while preserving the existing error for a true empty transaction request with no single-edit fields. Regression tests cover both provider-default shapes. Structured print output also records a bounded, already-redacted error string for failed tool calls, while successful calls still omit tool output.

The eval prompt now exposes the required verifier command equally to every agent. This removed repeated `python` versus `python3` guessing from the Python fixtures. A three-repeat imp/Pi repair regression passed 3/3 for both agents. The empty-array failure disappeared, but two imp runs exposed the empty-anchor bug before it was fixed. After both dispatch fixes, an imp-only confirmation passed with zero failed calls in 34.5s, seven provider requests, and 12,387 effective tokens.

First-attempt reliability is improved, but the confirmation still used seven successful provider rounds. The next optimization target is therefore successful-but-redundant discovery and closeout rounds, not local tool execution.

Repair regression report:

- `1783639780607-imp-vs-pi-0057d00bf4184cf697e00b28c6a81ce6.json`

## Recommended optimization order

1. Keep runtime-owned changed paths, failures, blockers, and verification debt.
2. Keep model-authored planning dormant for straightforward tasks.
3. Normalize optional provider defaults before edit-mode dispatch.
4. Expose concrete acceptance commands instead of making agents guess executable aliases.
5. Reduce successful-but-redundant discovery, verification, and closeout rounds.
6. Prefer one focused discovery pass and one verification pass for small repairs.
7. Re-run repeated imp/Pi comparisons after each loop change and reject verifier regressions.
8. Repeat the four-agent matrix before claiming a stable external ranking.
9. Add harder multi-file and recovery tasks before making the fast path the default globally.

The latency goal should be fewer provider requests, not faster local tools. A reasonable initial target is four requests for no-op tasks and at most six for focused one-file repairs while preserving the current 9/9 verifier pass rate.

## Small-model harness shakeout

A `gpt-5.6-luna` run through Pi on the promoted `DynamicCache` task proved useful as a harness test:

- Luna completed the task in about 250 seconds for a reported `$0.2875`.
- The verifier passed Ruff and semantic checks.
- The agent changed 16 files because Transformers regenerated model files from modular sources.
- The initial 12-file expectation incorrectly failed the run; the task now allows up to 20 files within the model-source allowlist.
- The run also exposed misleading failure composition: successful agent and verifier phases were formatted with failure-only messages when expectations failed.
- Direct imp invocation returned a provider 404 for Luna, while Pi and Codex could access it. Small-model shakeouts should use a compatible adapter until imp's provider route supports the model.
- Reusing a local Git object store avoided duplicating the roughly 469 MiB Transformers pack under low disk space.

This is harness evidence, not an imp-versus-baseline ranking. Small models are intentionally useful here because partial solutions and recoverable failures exercise artifact, verifier, timeout, and expectation paths cheaply.
