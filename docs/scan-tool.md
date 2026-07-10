# Scan tool technical reference

The `scan` tool gives the model a structured way to inspect local code without shelling out to `rg`, `find`, or language-specific CLIs for every question. It is read-only, tree-sitter-backed, and optimized for agent workflows: find symbols, extract enclosing code, identify nearby context, and suggest likely tests.

Implementation entrypoint:

- `crates/imp-core/src/tools/scan/mod.rs` — implementation and inline tests

Related implementation:

- `crates/imp-core/src/tools/scan/types.rs` — scan result/domain types
- `crates/imp-core/src/tools/scan/<language>.rs` — language-specific parsers
- `crates/imp-core/src/repo_index.rs` — structural search/related ranking over scan results
- `crates/imp-core/src/tools/code_intel.rs` — code block shape reused by extraction
- `crates/imp-tui/src/views/tool_output/` — TUI rendering for tool output

## Goals

`scan` is intended to be the default native tool for code-structure questions:

- Where is symbol `X` defined?
- What functions/types exist under this directory?
- Extract the enclosing function/class/module around a line or symbol.
- What tests are likely related to this target?
- What symbols are structurally related to this target?

It complements, rather than replaces, shell search:

- Use `scan` for structural lookup, symbol extraction, and likely tests.
- Use `bash`/`rg` for exact textual references, unsupported languages, generated files, or repository-specific command output.

## Public tool contract

The tool schema is declared by `ScanTool::parameters`.

Required parameter:

- `action`: one of `directory`, `files`, `extract`, `search`, `tests`, `related`

Common parameters:

- `directory`: directory to scan; defaults to the tool cwd
- `files`: explicit file list for scan/search/test/related actions
- `target`: single target for extraction or related/test lookup
- `targets`: multiple extraction targets
- `query`: search query or fallback target input
- `mode`: search mode, currently `symbol` or `text`
- `max_results`: result limit

The tool is read-only (`is_readonly() == true`) and returns `ToolOutput` with both human-readable text and structured `details` JSON.

Unsupported historical actions such as `references` and `impact` intentionally return guidance instead of silently doing an approximate operation. Agents should use `scan related`, `scan tests`, or `bash`/`rg` depending on intent.

## Actions

### `directory`

Scans source files under a directory and returns a structural summary. The action discovers files, parses supported languages, and emits grouped code structure.

Important behavior:

- Defaults to cwd when `directory` is omitted.
- Uses the same source-file discovery pipeline described below.
- Applies output truncation through scan's tool-output limits.
- Has a fast Rust-only skeleton path for all-`.rs` file sets.

Typical use:

```json
{
  "action": "directory",
  "directory": "crates/imp-core/src/tools"
}
```

### `files`

Scans an explicit file list. This bypasses directory discovery but still filters/parses according to supported language handling.

Typical use:

```json
{
  "action": "files",
  "files": ["crates/imp-core/src/tools/scan/mod.rs"]
}
```

### `extract`

Extracts code blocks for targets. Targets can be provided as `target` or `targets`.

Supported target forms:

- `path#symbol`
- `path:line`
- `path:start-end`

Extraction uses tree-sitter where possible to return the enclosing block around a line or named symbol. Invalid targets are reported in `details.errors` and set `is_error` when nothing useful could be extracted.

Typical use:

```json
{
  "action": "extract",
  "target": "crates/imp-core/src/tools/scan/mod.rs#collect_source_files"
}
```

### `search`

Searches symbols and structural metadata.

Flow:

1. Discover or resolve files.
2. Prefilter likely-relevant files for the query.
3. Parse files into `ScanResult`.
4. Build `RepoStructureIndex`.
5. Rank hits with repo-index scoring.
6. Fall back to the older symbol-index search path if repo-index search returns no useful hits.

Search results include:

- file path
- line
- symbol name
- kind
- score
- why/ranking reasons
- repo-intelligence summary

Typical use:

```json
{
  "action": "search",
  "query": "scan fallback ignore",
  "mode": "symbol",
  "max_results": 10
}
```

### `tests`

Suggests likely tests for a target. The implementation scans likely test files and symbols, then emits candidate test commands when it can infer them.

For Rust tests, candidates may include `cargo test <test_name>`. Results are heuristics, not a replacement for the project's real test command or CI configuration.

Typical use:

```json
{
  "action": "tests",
  "target": "crates/imp-core/src/tools/scan/mod.rs#collect_source_files",
  "max_results": 10
}
```

### `related`

Finds symbols related to a target.

Flow:

1. Discover or resolve files.
2. Prefilter related files around the target path/name.
3. Parse into `ScanResult`.
4. Build `RepoStructureIndex`.
5. Prefer edge-backed related symbols from the repo index.
6. Fall back to same-file structural context and test discovery when edge-backed context is unavailable.

Output is grouped into definition, related symbols, and tests where possible. Repo-index related entries include relationship/why labels.

Typical use:

```json
{
  "action": "related",
  "target": "crates/imp-core/src/tools/scan/mod.rs#collect_source_files"
}
```

## Source-file discovery

Source discovery is centralized in `collect_source_files`.

The pipeline is deliberately conservative:

1. If the root is a file, return it only if `is_supported(root)` is true.
2. If the root does not exist, return a tool error.
3. Try `git ls-files -z -- <pathspec>` from the target root.
4. If git listing is unavailable, use an ignore-aware filesystem fallback.

### Git fast path

`git_tracked_source_files` is preferred because it is fast, deterministic, and naturally honors the repository's tracked-file set. It filters the returned files through `is_supported`.

This means ignored or untracked files generally do not appear when scanning inside a git repository through the fast path. Agents that need untracked/generated files should use explicit `files` or shell search.

### Ignore-aware fallback

The fallback uses `ignore::WalkBuilder`.

Enabled behavior:

- `.gitignore`
- `.git/info/exclude`
- global git excludes
- `.ignore`
- no symlink following
- parallel walking using available CPU parallelism
- sorted result paths for stable behavior

The fallback detects whether the root is inside a git worktree with `git rev-parse --is-inside-work-tree`. For non-git roots it also hides dot paths and applies additional noisy-directory overrides.

### Non-git noisy-directory pruning

When scanning outside git metadata, `scan` excludes common dependency/build/cache directories so broad scans of home directories or extracted archives do not crawl large irrelevant trees.

Common ignored patterns include:

- `node_modules`
- `__pycache__`
- `venv`
- `.venv`
- `vendor`
- `dist`
- `build`
- `.next`
- `coverage`
- `target/debug`
- `target/release`
- `target/rust-analyzer`
- `target/criterion`

Platform-specific noisy areas are added behind `cfg`, such as macOS `Library/Caches`, `Library/Containers`, and related application-storage directories.

This policy is intentionally scoped to fallback discovery. The git fast path remains authoritative inside repositories.

## Language support

Supported languages are extension-dispatched. The user-facing supported language list is exposed through tool metadata and maintained near `SUPPORTED_LANGUAGES`.

Current supported language families include:

- Shell
- Python
- Rust
- JavaScript / TypeScript
- Go
- Elixir
- Ruby
- Perl
- Lua / Luajit
- Zig
- Odin
- Swift
- Kotlin
- Java
- C
- C#
- C++
- PHP
- Scala
- Dart
- OCaml

`is_supported` is extension-based. A file with an unsupported extension is not parsed even if its contents look like a supported language.

## Parsing and scan result model

`extract_files` is the main parse entrypoint. It accepts paths and cwd, returns `ScanResult`, and uses a small in-process cache keyed by file set.

Behavior:

- Small file sets are parsed serially.
- Larger file sets are parsed with rayon.
- Files that cannot be read as UTF-8 are skipped.
- Files containing NUL bytes are skipped.
- Each supported file is dispatched by extension to a language parser.

`ScanResult` aggregates:

- types/classes/modules/protocols/etc.
- functions/methods/procedures
- tests
- structural edges where parsers can infer them
- source/range metadata used by extraction and repo indexing

Language-specific parsers should preserve the shape expected by `RepoStructureIndex::from_scan_result` and existing scan output tests.

## Repo structure index integration

`RepoStructureIndex` adapts scan output into a graph-like local structure index.

It contains:

- `nodes`: symbols with identity, qualified name, location, search text, and test marker
- `edges`: structural relationships such as calls or containment where available
- freshness/workspace metadata

`scan search` uses this index first. Ranking considers:

- exact symbol-name match
- partial symbol-name match
- qualified-name match
- path match
- metadata/search-text match
- test-result penalty when the query does not ask for tests
- same-file saturation penalty to avoid one file dominating results

`scan related` also uses this index first, preferring edge-backed relationships before falling back to same-file context.

The repo index is currently built on demand from scan results. It is not a persistent background index in this tool path.

## Output and truncation

Scan output is optimized for model consumption:

- text output gives compact, line-oriented summaries
- `details` JSON preserves structured data for renderers and downstream logic
- output is truncated by `MAX_OUTPUT_LINES`, `MAX_OUTPUT_BYTES`, and line-length limits

Important constants live near the top of `scan/mod.rs`:

- `MAX_OUTPUT_LINES`
- `MAX_OUTPUT_BYTES`
- `MAX_LINE_CHARS`

TUI rendering has scan-specific formatting under `crates/imp-tui/src/views/tool_output/`.

## Caching and performance

Important performance choices:

- Prefer `git ls-files` for repository scans.
- Use `ignore::WalkBuilder` only as fallback discovery.
- Sort fallback paths for stability.
- Prefilter files before search/tests/related parsing.
- Parse larger file sets in parallel with rayon.
- Use a simple process-local file-set cache for `extract_files`.
- Use a fast Rust skeleton output path for some Rust-only directory summaries.

The cache is intentionally simple and in-memory. It does not watch filesystem changes and is not a durable repo index.

## Policy and safety

`scan` is read-only. It performs filesystem reads and local `git` discovery commands, but does not mutate files, git state, workflow state, or external services.

Security/safety properties:

- no symlink following in fallback walks
- supported-extension filtering before parsing
- unreadable/binary-ish files are skipped
- broad non-git scans avoid obvious dependency/cache directories
- output truncation limits prevent unbounded tool responses

## Testing

Primary tests live in `crates/imp-core/src/tools/scan/mod.rs` alongside the implementation.

Useful targeted commands:

```sh
cargo test -p imp-core collect_source_files --lib
cargo test -p imp-core scan_non_git_ignore --lib
cargo test -p imp-core scan_tool --lib
cargo test -p imp-core scan_search_repo_index --lib
cargo test -p imp-core scan_related_repo_index --lib
cargo test -p imp-core rust_scan_edge --lib
cargo test -p imp-core repo_index --lib
```

Formatting/checking:

```sh
cargo fmt --check
cargo check -p imp-core
```

When unrelated dirty files block full formatting, verify the touched file with targeted rustfmt and record the limitation:

```sh
rustfmt --edition 2021 --check crates/imp-core/src/tools/scan/mod.rs
```

## Extension guidelines

When adding scan behavior:

- Preserve the public tool schema unless the UX explicitly needs a schema change.
- Keep `details` JSON keys stable where existing model/TUI behavior depends on them.
- Prefer adding parser coverage behind tests with representative source snippets.
- Keep file discovery conservative; do not broaden scans into generated/dependency trees without a product reason.
- Use explicit fallback behavior rather than silently returning misleading partial results.
- Add targeted tests for new language parsers, discovery behavior, ranking behavior, and extraction edge cases.
- Do not add persistent indexing, watchers, or background lifecycle to this one-shot tool path without a separate design.
