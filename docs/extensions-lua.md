# Lua extensions

Lua is the shipped extension runtime for imp.

Primary implementation areas:

- `crates/imp-lua/src/loader.rs`
- `crates/imp-lua/src/bridge.rs`
- `crates/imp-lua/src/sandbox.rs`
- `crates/imp-core/src/config.rs`

## Load paths

```text
~/.imp/lua/
<project>/.imp/lua/
```

A Lua extension can be either:

- a legacy standalone `.lua` file; or
- a directory containing `init.lua` and, optionally, `manifest.lua`.

Directory extensions are the preferred shape for new work because they provide a stable extension identity for namespaced commands and future capability declarations.

```text
~/.imp/lua/github/
  manifest.lua
  init.lua
```

## Extension manifests

A directory extension can include `manifest.lua`:

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

The manifest `name` is the extension identity. Commands registered while that extension loads are exposed as `extension.command`, so the example above can be invoked as:

```text
/x github.review-pr
```

If `manifest.lua` is missing or invalid, imp falls back to the extension directory name. Standalone `.lua` files use their file stem as the extension name.

## Capabilities

Extension capability policy controls access to:

- shell execution
- filesystem access
- HTTP
- secrets
- native imp tools
- UI prompts

Use the narrowest policy that supports the extension.

## Commands

New extensions should use the module form and command alias:

```lua
local imp = require("imp")

imp.command("review-pr", {
    description = "Review the current PR",
    run = function(args)
        return "Reviewing PR: " .. (args or "current")
    end,
})
```

`imp.command(name, def)` is an alias for `imp.register_command(name, def)`. Command definitions may use either `run` or `handler`.

Legacy command registration still works:

```lua
imp.register_command("greet", {
    description = "Say hello",
    handler = function(args)
        return "Hello, " .. (args or "world")
    end
})
```

Extension commands appear under the TUI slash namespace:

```text
/x <extension.command> [args]
/ext <extension.command> [args]
/lua <extension.command> [args]
```

Unqualified command names continue to work when they are unambiguous. If two extensions register the same raw command name, use the extension-qualified name.

## Tools

```lua
imp.register_tool({
    name = "echo_custom",
    description = "Echo text",
    execute = function(call_id, params, ctx)
        return { text = params.text }
    end
})
```

Tool handlers receive a call id, parameter table, and context table. Context includes cwd and cancellation state.

## Hooks

Lua extensions can register handlers for runtime events through the host API. Hooks should be kept small and deterministic where possible.

## Stability

Lua is the current shipped extension path. TypeScript extension support exists in repository code paths but should not be documented as the stable shipped extension system.
