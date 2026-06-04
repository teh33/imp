pub mod bridge;
pub mod loader;
pub mod sandbox;

use std::path::Path;
use std::sync::{Arc, Mutex};

use imp_core::config::LuaCapabilityPolicy;
use imp_core::tools::ToolRegistry;

pub use bridge::{json_to_lua_value, load_lua_tools, lua_value_to_json, setup_host_api, LuaTool};
pub use loader::{discover_extensions, load_extensions, reload, LuaExtension};
pub use sandbox::{
    LuaCallContext, LuaCommandHandle, LuaError, LuaHookHandle, LuaRuntime, LuaToolHandle,
};

/// Discover and load Lua extensions from user and project directories,
/// registering any tools they define onto the given registry.
///
/// Returns the shared runtime handle (for command dispatch and hot-reload).
/// Returns `None` if no extensions were found or the runtime failed to start.
pub fn init_lua_extensions(
    user_config_dir: &Path,
    project_dir: Option<&Path>,
    tools: &mut ToolRegistry,
    policy: &LuaCapabilityPolicy,
) -> Option<Arc<Mutex<LuaRuntime>>> {
    let extensions = discover_extensions(user_config_dir, project_dir);
    if extensions.is_empty() {
        return None;
    }

    let rt = match LuaRuntime::new() {
        Ok(rt) => rt,
        Err(_e) => {
            return None;
        }
    };
    if let Err(_e) = setup_host_api(&rt) {
        return None;
    }
    rt.apply_capability_policy(policy);

    let results = load_extensions(&rt, &extensions);
    for (_name, result) in &results {
        if let Err(_e) = result {
            // Keep extension bootstrap silent in embedded runtimes; failed extensions
            // simply do not register their tools.
        }
    }

    // Give the Lua runtime access to native tools for imp.tool() calls
    rt.set_native_tools(tools.tools_map());

    let runtime = Arc::new(Mutex::new(rt));
    load_lua_tools(Arc::clone(&runtime), tools);
    Some(runtime)
}

#[cfg(test)]
mod tests;
