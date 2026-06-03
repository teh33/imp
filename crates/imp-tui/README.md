# imp-tui

`imp-tui` is the fullscreen terminal interface for [imp](https://github.com/kfcafe/imp).

It provides the cockpit UI used by the `imp` binary: message stream, editor, command palette, session views, model/thinking controls, settings screens, tool-call inspection, and runtime event rendering.

## What this crate provides

- Ratatui-based terminal application
- prompt editor and input history
- command palette and file attachment UI
- model selector and thinking-level controls
- session tree and branch navigation views
- sidebar inspection for tool calls and outputs
- settings and secrets screens
- rendering for agent/runtime events

## Intended use

Most users should run the `imp` binary rather than depend on `imp-tui` directly:

```bash
imp
```

This crate is useful if you are working on imp's terminal interface or building another Rust entrypoint around the same UI components.

## Status

The terminal UI is an active user-facing surface of imp and is under active development. Internal APIs may change as the app is decomposed and the runtime/UI boundary improves.

## Repository

- Main README: <https://github.com/kfcafe/imp>
- Crate: <https://crates.io/crates/imp-tui>
