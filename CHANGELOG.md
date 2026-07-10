# Changelog

All notable changes to `imp` are documented here.

The format is inspired by [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). `imp` is actively developed, so entries should be concise, user-facing, and grouped by release.

## [Unreleased]

## [0.3.5] - 2026-07-10

### Added

- Added Kimi K2.7 model metadata, aliases, and provider-specific thinking behavior.
- Added runtime-owned task state with task lifecycle tools and durable evidence projection.
- Added typed audit configuration, requirement discovery, doctor reporting, and finding summaries.

### Changed

- Reduced TUI clutter by attaching policy, trust, and provenance notices to tool calls, compacting warning and error presentation, and bounding shell output by rendered rows.
- Reduced idle TUI rendering overhead and excluded nested Git repositories and worktrees from repository statistics.
- Split the edit tool into focused matching, exact-edit, anchored-edit, and output modules.

### Fixed

- Listed resumable sessions consistently across configured storage roots.
- Exposed current OAuth-backed models in the TUI model selector.

## [0.3.4] - 2026-07-09

### Added

- Added GPT-5.6 model metadata and aliases, including Sol, Terra, and Luna variants.

### Changed

- Reduced default prompt/request size by removing the dynamic Current Task State system-prompt block.
- Shortened the model-facing `read` tool schema while preserving existing runtime compatibility for advanced read options.

## [0.3.3] - 2026-06-03

### Added

- Added a technical reference for the native `scan` tool and linked it from the docs index.

### Changed

- Removed the unused personality profile/slider system while preserving authored soul customization and the fixed professional default identity.
- Simplified imp-specific agent instructions and removed stale mana/Tower architecture guidance.

## [0.3.2] - 2026-06-03

### Changed

- Improved workflow run readiness reporting with clearer blocked-step reasons and safe parallel subagent batch suggestions.
- Updated TUI context reporting and compaction handling so active context usage is easier to understand.

### Fixed

- Surfaced pre-output agent/provider failures in the TUI instead of leaving chats idle with an empty assistant turn.
- Added bounded recovery for pre-output context exhaustion by masking tool outputs and retrying once when safe.
- Treated provider terminal errors as failed turns and avoided phantom workflow child runs.

### Internal

- Added cache-aware benchmark output for runtime usage analysis.

## [0.3.1] - 2026-06-02

### Changed

- Decomposed the TUI app root into focused runtime, rendering, input, command, settings, welcome, diagnostics, lifecycle, and test modules for maintainability while preserving behavior.
- Removed legacy mana integration from the active public surface; native file-backed workflows are now the durable orchestration path.
- Removed unused direct `mana-core` dependency from the experimental `imp-gui` crate.
- Clarified that the ACP editor adapter scaffold is internal/out-of-scope for 0.3.0 unless separately verified.

## [0.3.0] - 2026-05-29

### Added

- Added actionable workflow run contracts so command-backed workflows can publish clearer execution and verification expectations.
- Added cached repo intelligence summaries and more visible startup metadata for project context.

### Changed

- Sped up TUI startup and session picker loading, including faster recent-session scans and cached repository intelligence.
- Simplified agent runtime stop policy so tool results and turn continuation are handled more predictably.
- Treat bare CLI arguments as one-shot prompts for smoother command-line usage.
- Increased Tokio worker stack size for deeper async/runtime paths.

### Fixed

- Fixed startup skill hit detection.
- Handled repeated key events in the TUI.

## [0.2.9] - 2026-05-28

### Added

- Added executable workflow runner support for command-backed checks, step status advancement, and reconciliation dogfood workflows.
- Added repo intelligence counts to scan output and TUI startup/homepage surfaces.
- Added dependency audit notes for current RustSec findings and blocked upgrade paths.

### Changed

- Prefer native workflow policy, prompt, and tool surfaces over legacy mana/prototype guidance.
- Bound helper command execution for guardrails, hooks, git, shell tools, Lua/TypeScript extension bridges, and compaction requests.
- Improved workflow tool output cards and related TUI rendering polish.

### Fixed

- Reconciled workflow status after command checks so stale workflow states are easier to detect and clean up.
- Redacted secrets from persisted diagnostics, traces, provider/search errors, web-read URLs, and truncated command output files.
- Tightened write-path checks around symlink escapes and preserved private permissions on session and trace files.

## [0.2.8] - 2026-05-27

### Added

- Added the native `workflow` tool for creating and running structured agent workflows from imp.
- Added workflow boundary documentation and technical reference pages for architecture, sessions, tools, policy, RPC, Lua extensions, and workflows.

### Changed

- Made TUI selection copy use `Cmd+C`, leaving `Ctrl+C` reserved for clear, abort, and quit behavior.

### Fixed

- Fixed compaction fail-fast behavior and reuse-bench context initialization.
- Bumped the workspace release version to `0.2.8`.

## [0.2.7] - 2026-05-26

### Changed

- Made the TUI session tree denser, newest-first, and more readable on smaller terminals, with inline position counts and preview shown only when there is ample space.
- Bumped the workspace release version to `0.2.7`.

## [0.2.6] - 2026-05-26

### Added

- Added bounded subagent runtime primitives in `imp-core`, including subagent DTOs, lifecycle/status/activity-summary/outcome structures, and an initial no-op coordinator for future bounded worker orchestration.
- Added design documentation for bounded subagent orchestration and the imp runtime boundary around clean worker contexts, activity summaries, cancellation, evidence, and host-owned durable state.
- Added Codex/OpenAI-compatible prompt cache identity wiring so supported providers can receive stable cache hints.

### Changed

- Made `imp` standalone by default again: the default `imp-cli` dependency tree no longer pulls in `imp-work`, `mana-core`, or `mana-cli`.
- Moved mana-facing CLI/TUI/tool integration behind optional `mana-ui` / `mana-tool` feature gates while keeping default chat/run usage independent from mana.
- Split `imp-core` workflow integration away from the core agent loop into explicit runtime layers, separating recipe/runtime support from mana compatibility code.
- Reworked mana-related runtime hooks so mana API support and the heavier mana tool/CLI integration are separate optional feature surfaces.
- Updated the TUI to compile in a standalone default configuration, with mana navigator/run UI available only when the optional mana UI feature is enabled.
- Bumped the workspace release version to `0.2.6`.

### Removed

- Removed active `imp-work` workspace membership and default `imp-core` / `imp-cli` dependencies on `imp-work`.
- Removed the native `work` tool from the default imp runtime surface; durable work orchestration is no longer part of the default standalone imp build.
- Removed the legacy native `imp run` / WorkRun planning path from the default CLI surface.

### Fixed

- Fixed feature gating so `imp-core --features mana-api` builds without pulling in the heavier mana tool integration.
- Fixed default dependency hygiene so `cargo tree -p imp-cli` has no `imp-work`, `mana-core`, or `mana-cli` entries unless optional mana integration is enabled.
- Fixed standalone TUI/CLI build paths that previously assumed mana run state or mana navigator types were always available.
- Fixed TUI tool icon label spacing.

## [0.2.5] - 2026-05-22

### Added

- Added autonomous runtime objective and obligation tracking for failed-command recovery and edited-file verification.

### Changed

- Made imp-work follow-up tasks created from outcomes ready by default so active continuation work remains discoverable.
- Made work tool operations honor explicit project paths for global project-scoped stores.

### Fixed

- Prevented explanation-only answers from creating durable imp-work tasks unless the user explicitly asks for durable work structure.
- Blocked parent task close/outcome when open child tasks remain, unless explicitly forced.
- Made `work(action="next")` report todo child tasks so agents do not confuse “no ready tasks” with “no work remains.”
- Fixed imp-work validation and discovery to account for merged global project-scoped and local task state.

## [0.2.4] - 2026-05-22

### Added

- Added native `imp run <work-id> --dry-run` planning for imp-work task and epic dispatch, including dependency, context, and path-conflict blockers.
- Added `imp stats` reports for local session, token, cost, tool, file-change, project, and wrapped-style usage summaries.
- Added persisted imp-work coordinator run records and event logs for future multi-agent work orchestration.
- Added structured file and line-change metadata to read/write/edit tool results so UI, stats, evidence, and handoff surfaces can summarize tool effects reliably.
- Added richer TUI tool cards, tool icons, command-palette pages for commands/skills/workflows, and sidebar detail cards for common tools.
- Added design planning docs for Droid parity, mission-mode vs imp-work, host sync, and OSS launch readiness.

### Changed

- Folded worktree management into the native `git` tool as `worktree_list`, `worktree_add`, and `worktree_remove`; removed the standalone `worktree` tool.
- Removed the legacy `session_search`/`recall` native tool from the default registry and mode tool lists.
- Tightened autonomous continuation behavior so failed shell commands are treated as recoverable obligations while durable-work externalization is only nudged when the user explicitly asks for durable work structure.
- Refreshed the README around the current local-first imp surface, native imp-work, providers, safety controls, and extension support.
- Standardized TUI tool label rendering so icons sit directly next to names, including `$Terminal`.

### Fixed

- Fixed sidebar and chat rendering expectations around the new tool card headers and summary detail lines.
- Fixed readable imp-work task IDs to be deterministic from titles and deduplicated with numeric suffixes.

## [0.2.3] - 2026-05-22

### Added

- Added the native `work` tool as imp's durable work system, replacing mana as the default agent workflow for tasks, lifecycle state, context, verification, and handoff.
- Added global project-scoped imp-work storage under `~/.imp/work`, with normal work actions keyed by canonical project root instead of cwd-local `.imp/work`.
- Added `work(action="guide")` for agent-facing imp-work operating guidance and `work(action="scope")` for explicit store/source visibility.
- Added project stream events and automatic stream-history loading into task context packs so follow-up work can retain continuity after prior tasks close.
- Added an offline `scripts/migrate-mana-to-imp-work` migration command for importing existing `.mana` units into global imp-work storage.
- Added Z.AI provider/model metadata and aliases for GLM models.

### Changed

- Switched system prompt, mode guidance, and agent workflow nudges from mana-first durable work to native imp-work.
- Made global imp-work the normal backend for work create/list/show/update and lifecycle graph flows; project-local work stores are now migration input only.
- Moved migration out of the normal work tool API so migration remains transitional tooling rather than an everyday agent action.

### Fixed

- Kept imp-work lifecycle, verification, dependency, context, and tree flows coherent after the global-only backend switch.

## [0.2.2] - 2026-05-21

### Fixed

- Fixed over-eager runtime stopping after tool observations so `read`, `edit`, failed `bash`, and mana close results are interpreted by the agent instead of ending work prematurely.

### Added

- Added GitHub as a `web` search/read provider and wired GitHub credential setup through app surfaces.
- Added constrained `--print` worker controls for allowing/denying tools and write paths, plus JSON final output.

### Changed

- Prioritized current-project sessions in the session picker.
- Improved native mana run responsiveness during direct run orchestration.

## [0.1.3] - Unreleased

### Added

- Added a native `worktree` tool for creating, listing, and removing git worktrees separately from the general `git` tool.
- Removed defunct `ask_agent` helper-agent tool; use mana units/runs for durable delegation.
- Added `ask_user` schema support, including TUI multi-select prompts.
- Added mana `guide` and `template` actions so agents can inspect task/epic/decision/verify/orchestration guidance from the native mana tool.
- Added validation hints to mana actions to make schema errors and missing arguments easier for agents to recover from.
- Added skill-provided slash command discovery and command-palette display for Lua slash commands.
- Added `/update-imp` as the local Lua updater command for bumping, pushing, installing, and verifying nightly imp builds.

### Changed

- Renamed the `shell` tool to `bash` and simplified secret injection for shell commands.
- Simplified `git`, `scan`, `edit`, and related tool schemas and metadata so agents receive cleaner, more bounded tool interfaces.
- Made anchored edits a first-class edit mode and added explicit line-range support to `read`.
- Made TUI `/compact` run asynchronously with visible progress instead of blocking the interface.
- Made Lua slash commands run with the same background progress interaction as `/compact` instead of blocking the TUI.
- Strengthened mana worker context assembly and improved mana unit closure reliability after implementation.
- Switched project licensing metadata and repository license text to MPL-2.0.

### Fixed

- Hardened web read/search schema handling, URL read safety, and YouTube metadata/transcript extraction paths.
- Added clearer write overwrite behavior without expanding the write schema surface.
- Removed stale spawn references after the helper-agent redesign.
- Improved prompt wrapping, selected-tool indicators, tool-call inspector styling, and startup version display in the TUI.
- Deduplicated Lua extension loading and startup command surfacing so commands such as `/update-imp` appear once.
- Fixed Lua command reload policy handling and argument execution so reloaded extensions preserve configured capabilities and pass argv without shell joining.

### Documentation

- Documented Lua slash command behavior in the README.
- Added skill-writing reference updates.

### Internal

- Continued schema hardening across core tools, including `read`, `write`, `edit`, `scan`, `git`, `bash`, `web`, `ask_user`, and mana.

## [0.1.2] - 2026-04-28

### Fixed

- Deduplicated discovered instruction files so identical global `agents.md`/`AGENTS.md` content is not injected repeatedly into trivial prompts.
- Avoided reinjecting the same global `.imp/agents.md` when it is also discovered while walking ancestor project directories.

## [0.1.1] - 2026-04-28

### Documentation

- Expanded crate-level README files for published crates:
  - `imp-llm`
  - `imp-core`
  - `imp-lua`
  - `imp-tui`
  - `imp-cli`
- Added crate-specific descriptions, intended-use notes, status sections, and repository links for crates.io readers.

## [0.1.0] - 2026-04-28

Initial crates.io release of the imp crate family.

### Added

- Published `imp-llm`, the provider and streaming client layer.
- Published `imp-core`, the agent runtime with tools, sessions, context, hooks, mana integration, and early SDK surface.
- Published `imp-lua`, the Lua extension runtime.
- Published `imp-tui`, the fullscreen terminal interface.
- Published `imp-cli`, the command-line entrypoint and `imp` binary crate.

### Features

- Terminal UI and CLI chat entrypoints.
- Native tools for file I/O, editing, shell commands, git, structural code scanning, web reading/search, user prompts, memory, and mana coordination.
- Durable JSONL sessions with branch metadata and context compaction support.
- Provider support for Anthropic, OpenAI, Google, and OpenAI-compatible services.
- Secure credential storage through OS credential stores.
- Agent modes for full, worker, orchestrator, planner, reviewer, and auditor workflows.
- Lua extension support for tools, commands, and hooks.
- Direct mana task execution through `imp run <unit-id>`.

### Documentation

- Reworked the main README to position imp as an extensible coding agent built for efficiency and performance.
- Added plain-language documentation for mana as included durable task coordination.
- Documented install, quick start, tools, sessions, providers, customization, architecture, and CLI reference.

### Internal

- Standardized published crates on MIT license metadata.
- Added crates.io metadata and versioned internal dependencies for published crates.

[Unreleased]: https://github.com/kfcafe/imp/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/kfcafe/imp/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/kfcafe/imp/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/kfcafe/imp/compare/v0.2.9...v0.3.0
[0.2.9]: https://github.com/kfcafe/imp/compare/v0.2.8...v0.2.9
[0.2.8]: https://github.com/kfcafe/imp/compare/v0.2.7...v0.2.8
[0.2.7]: https://github.com/kfcafe/imp/compare/v0.2.6...v0.2.7
[0.2.6]: https://github.com/kfcafe/imp/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/kfcafe/imp/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/kfcafe/imp/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/kfcafe/imp/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/kfcafe/imp/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/kfcafe/imp/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/kfcafe/imp/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/kfcafe/imp/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/kfcafe/imp/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/kfcafe/imp/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kfcafe/imp/releases/tag/v0.1.0
