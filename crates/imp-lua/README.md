# imp-lua

`imp-lua` is the Lua extension runtime for [imp](https://github.com/kfcafe/imp).

It provides the bridge between imp's Rust runtime and user/project Lua scripts that register tools, commands, and hooks.

## What this crate provides

- Lua runtime loading through `mlua`
- extension discovery and execution support
- bridge APIs for registering tools and commands
- hook integration for runtime events
- capability-aware access to shell, filesystem, HTTP, secrets, and native imp tools

## Intended use

Most users interact with this crate by writing Lua files for imp rather than depending on `imp-lua` directly.

Lua extension load paths:

- `~/.imp/lua/`
- `<project>/.imp/lua/`

New extensions should prefer a directory package with `manifest.lua` and `init.lua`:

```text
~/.imp/lua/github/
  manifest.lua
  init.lua
```

`manifest.lua`:

```lua
return {
    name = "github",
    version = "0.1.0",
    description = "GitHub helper commands",
    commands = {
        { name = "review-pr", description = "Review the current PR" },
    },
}
```

`init.lua`:

```lua
local imp = require("imp")

imp.command("review-pr", {
    description = "Review the current PR",
    run = function(args) return "Reviewing " .. (args or "current PR") end,
})
```

The manifest name becomes the command namespace. Invoke the command from the TUI as:

```text
/x github.review-pr
```

Legacy standalone files and `imp.register_command` continue to work:

```lua
imp.register_command("greet", {
    description = "Say hello",
    handler = function(args) return "Hello, " .. (args or "world") end
})
```

## Status

Lua is the current stable shipped extension path for imp. TypeScript compatibility exists elsewhere in imp, but it is more limited and still evolving.

## Repository

- Main README: <https://github.com/kfcafe/imp>
- Crate: <https://crates.io/crates/imp-lua>
