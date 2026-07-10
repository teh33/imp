# imp bug: loopr child result contains raw tool transcript and tool-loop noise

Status: filed from Core0 loopr/imp orchestration testing.

## Summary

When `imp` is used as a loopr exec child agent, loopr now correctly exports task/result/status paths and receives `result.md`, but the child result can contain raw tool transcript and tool errors instead of a clean final answer. The agent also made unnecessary/invalid local-file tool calls during a simple review task.

This appears to be an `imp` output/tool-routing issue, not a current loopr issue.

## Environment

- Project: `/Users/asher/core0`
- loopr run: `run_1782071441966`
- Thread example: `thr_013`
- Wrapper command shape:

```sh
imp --role "$role" --autonomy safe --max-turns 30 --no-session --output text -p "$prompt"
```

## Expected behavior

With `--output text`, `imp` should write only the final assistant answer to stdout, suitable for loopr to store as `result.md`.

For local repository review tasks, `imp` should use local file/search tools correctly and avoid repeated identical reads or invalid web/file URL attempts.

## Actual behavior

A loopr child review result included raw tool transcript and tool errors such as:

```text
[tool: web]
[error: Invalid URL: relative URL without a base]
[tool: web]
[error: Unsafe URL: unsupported URL scheme: file]
[tool: read crates/core0-kernel/Cargo.toml]
[tool: read crates/core0-kernel/Cargo.toml]
[tool: read crates/core0-kernel/Cargo.toml]
[tool: read crates/core0-kernel/Cargo.toml]
[error: Blocked: identical tool call repeated 4 times in a row for 'read']
```

In a later smoke test, `imp` completed and loopr accepted the result, but the result still contained tool transcript before the final response.

## Impact

Loopr orchestration now works, but imp child-agent review output is noisy and unreliable for automated review/fix loops. Parent agents must manually inspect and filter result files.

## Suggested fixes

- Ensure `--output text` suppresses tool transcript and returns only final assistant content.
- Avoid routing local/relative repository paths through `web`.
- Improve recovery when repeated identical tool calls are blocked.
- Consider a child-agent/loopr mode optimized for clean `result.md` output.
- Add an integration test where `imp` is invoked by a wrapper and must produce a clean final report for a local repo task.
