use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use imp_core::config::{AgentMode, Config, LuaCapabilityPolicy};
use imp_core::tools::{FileCache, FileTracker, Tool, ToolContext, ToolUpdate};
use imp_core::ui::UserInterface;
use mlua::Lua;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LuaError {
    #[error("Lua error: {0}")]
    Mlua(#[from] mlua::Error),

    #[error("Extension error: {0}")]
    Extension(String),
}

/// Handle to a Lua-registered tool.
pub struct LuaToolHandle {
    pub name: String,
    pub label: String,
    pub description: String,
    pub readonly: bool,
    pub params: serde_json::Value,
    /// Registry key for the execute function stored in Lua.
    pub execute_key: mlua::RegistryKey,
}

/// Handle to a Lua-registered hook.
pub struct LuaHookHandle {
    pub event: String,
    /// Registry key for the handler function stored in Lua.
    pub handler_key: mlua::RegistryKey,
}

/// Handle to a Lua-registered command.
pub struct LuaCommandHandle {
    pub name: String,
    pub extension: Option<String>,
    pub description: String,
    pub handler_key: mlua::RegistryKey,
}

impl LuaCommandHandle {
    pub fn canonical_name(&self) -> String {
        match self.extension.as_deref() {
            Some(extension) if !extension.trim().is_empty() => format!("{extension}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// Context passed to Lua host API functions during tool execution.
///
/// Mirrors `ToolContext` but is stored separately so the Lua
/// `imp.tool()` callback can construct a fresh `ToolContext` for
/// each native tool call.
pub struct LuaCallContext {
    pub cwd: PathBuf,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    pub update_tx: tokio::sync::mpsc::Sender<ToolUpdate>,
    pub command_tx: tokio::sync::mpsc::Sender<imp_core::agent::AgentCommand>,
    pub ui: Arc<dyn UserInterface>,
    pub file_cache: Arc<FileCache>,
    pub checkpoint_state: Arc<imp_core::tools::CheckpointState>,
    pub file_tracker: Arc<std::sync::Mutex<FileTracker>>,
    pub anchor_store: Arc<imp_core::tools::AnchorStore>,
    pub lua_tool_loader: Option<imp_core::tools::LuaToolLoader>,
    pub mode: AgentMode,
    pub read_max_lines: usize,
    pub run_policy: imp_core::policy::RunPolicy,
    pub config: Arc<Config>,
}

impl LuaCallContext {
    /// Build a `ToolContext` from the stored fields.
    pub fn to_tool_context(&self) -> ToolContext {
        ToolContext {
            cwd: self.cwd.clone(),
            cancelled: Arc::clone(&self.cancelled),
            update_tx: self.update_tx.clone(),
            command_tx: self.command_tx.clone(),
            ui: Arc::clone(&self.ui),
            file_cache: Arc::clone(&self.file_cache),
            checkpoint_state: Arc::clone(&self.checkpoint_state),
            file_tracker: Arc::clone(&self.file_tracker),
            anchor_store: Arc::clone(&self.anchor_store),
            lua_tool_loader: self.lua_tool_loader.clone(),
            mode: self.mode,
            read_max_lines: self.read_max_lines,
            turn_workflow_review: Arc::new(std::sync::Mutex::new(
                imp_core::workflow_review::TurnWorkflowReviewAccumulator::default(),
            )),
            run_policy: self.run_policy.clone(),
            config: Arc::clone(&self.config),
            supporting_provenance: Vec::new(),
        }
    }
}

impl From<ToolContext> for LuaCallContext {
    fn from(ctx: ToolContext) -> Self {
        Self {
            cwd: ctx.cwd,
            cancelled: ctx.cancelled,
            update_tx: ctx.update_tx,
            command_tx: ctx.command_tx,
            ui: ctx.ui,
            file_cache: ctx.file_cache,
            checkpoint_state: ctx.checkpoint_state,
            file_tracker: ctx.file_tracker,
            anchor_store: ctx.anchor_store,
            lua_tool_loader: ctx.lua_tool_loader,
            mode: ctx.mode,
            read_max_lines: ctx.read_max_lines,
            run_policy: ctx.run_policy,
            config: ctx.config,
        }
    }
}

/// Manages the Lua state for extensions.
pub struct LuaRuntime {
    lua: Lua,
    tools: Arc<Mutex<Vec<LuaToolHandle>>>,
    hooks: Arc<Mutex<Vec<LuaHookHandle>>>,
    commands: Arc<Mutex<Vec<LuaCommandHandle>>>,
    /// Extension currently being loaded; used to attach ownership to registrations.
    current_extension: Arc<Mutex<Option<String>>>,
    /// Native imp tools available via `imp.tool()` from Lua.
    native_tools: Arc<Mutex<HashMap<String, Arc<dyn Tool>>>>,
    /// Active execution context for `imp.tool()` calls.
    call_context: Arc<Mutex<Option<LuaCallContext>>>,
    /// Env vars this extension is allowed to read via `imp.env()`.
    allowed_env: Arc<Mutex<HashSet<String>>>,
    /// Whether `imp.tool()` calls are currently permitted.
    allow_native_tool_calls: Arc<AtomicBool>,
    /// Whether `imp.exec()` shell execution is permitted.
    allow_shell_exec: Arc<AtomicBool>,
    /// Whether `imp.http.*` calls are permitted.
    allow_http: Arc<AtomicBool>,
    /// Whether secret access is permitted.
    allow_secrets: Arc<AtomicBool>,
}

impl LuaRuntime {
    /// Create a new Lua runtime with standard libraries.
    pub fn new() -> Result<Self, LuaError> {
        let lua = Lua::new();
        Ok(Self {
            lua,
            tools: Arc::new(Mutex::new(Vec::new())),
            hooks: Arc::new(Mutex::new(Vec::new())),
            commands: Arc::new(Mutex::new(Vec::new())),
            current_extension: Arc::new(Mutex::new(None)),
            native_tools: Arc::new(Mutex::new(HashMap::new())),
            call_context: Arc::new(Mutex::new(None)),
            allowed_env: Arc::new(Mutex::new(HashSet::new())),
            allow_native_tool_calls: Arc::new(AtomicBool::new(true)),
            allow_shell_exec: Arc::new(AtomicBool::new(false)),
            allow_http: Arc::new(AtomicBool::new(false)),
            allow_secrets: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get a reference to the underlying Lua state.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Get a clone of the tools handle for external access.
    pub fn tools(&self) -> Arc<Mutex<Vec<LuaToolHandle>>> {
        Arc::clone(&self.tools)
    }

    /// Get a clone of the hooks handle for external access.
    pub fn hooks(&self) -> Arc<Mutex<Vec<LuaHookHandle>>> {
        Arc::clone(&self.hooks)
    }

    /// Get a clone of the commands handle for external access.
    pub fn commands(&self) -> Arc<Mutex<Vec<LuaCommandHandle>>> {
        Arc::clone(&self.commands)
    }

    /// Get a clone of the current extension handle.
    pub fn current_extension(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.current_extension)
    }

    /// Set the extension currently being loaded.
    pub fn set_current_extension(&self, extension: Option<String>) {
        *self.current_extension.lock().unwrap() = extension;
    }

    /// Get a clone of the native tools map.
    pub fn native_tools(&self) -> Arc<Mutex<HashMap<String, Arc<dyn Tool>>>> {
        Arc::clone(&self.native_tools)
    }

    /// Get a clone of the call context handle.
    pub fn call_context(&self) -> Arc<Mutex<Option<LuaCallContext>>> {
        Arc::clone(&self.call_context)
    }

    /// Get a clone of the allowed-env handle.
    pub fn allowed_env(&self) -> Arc<Mutex<HashSet<String>>> {
        Arc::clone(&self.allowed_env)
    }

    /// Get whether `imp.exec()` calls are currently permitted.
    pub fn allow_shell_exec(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.allow_shell_exec)
    }

    /// Get whether `imp.http.*` calls are currently permitted.
    pub fn allow_http(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.allow_http)
    }

    /// Get whether secret access is currently permitted.
    pub fn allow_secrets(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.allow_secrets)
    }

    /// Get whether `imp.tool()` calls are currently permitted.
    pub fn allow_native_tool_calls(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.allow_native_tool_calls)
    }

    /// Populate the native tool registry (called once after tools are registered).
    pub fn set_native_tools(&self, tools: HashMap<String, Arc<dyn Tool>>) {
        *self.native_tools.lock().unwrap() = tools;
    }

    /// Set the call context before executing a Lua tool function.
    pub fn set_call_context(&self, ctx: LuaCallContext) {
        *self.call_context.lock().unwrap() = Some(ctx);
    }

    /// Clear the call context after execution.
    pub fn clear_call_context(&self) {
        *self.call_context.lock().unwrap() = None;
    }

    /// Set the allowed env vars for this extension.
    pub fn set_allowed_env(&self, vars: HashSet<String>) {
        *self.allowed_env.lock().unwrap() = vars;
    }

    /// Set whether `imp.exec()` calls are permitted for the current runtime.
    pub fn set_allow_shell_exec(&self, allowed: bool) {
        self.allow_shell_exec.store(allowed, Ordering::Relaxed);
    }

    /// Set whether `imp.http.*` calls are permitted for the current runtime.
    pub fn set_allow_http(&self, allowed: bool) {
        self.allow_http.store(allowed, Ordering::Relaxed);
    }

    /// Set whether secret access is permitted for the current runtime.
    pub fn set_allow_secrets(&self, allowed: bool) {
        self.allow_secrets.store(allowed, Ordering::Relaxed);
    }

    /// Set whether `imp.tool()` calls are permitted for the current runtime.
    pub fn set_allow_native_tool_calls(&self, allowed: bool) {
        self.allow_native_tool_calls
            .store(allowed, Ordering::Relaxed);
    }

    /// Apply a shipped-runtime capability policy.
    pub fn apply_capability_policy(&self, policy: &LuaCapabilityPolicy) {
        self.set_allow_native_tool_calls(policy.allow_native_tool_calls);
        self.set_allow_shell_exec(policy.allow_shell_exec);
        self.set_allow_http(policy.allow_http);
        self.set_allow_secrets(policy.allow_secrets);
        self.set_allowed_env(policy.allowed_env.clone());
    }

    /// Register a tool handle (called from bridge).
    pub fn register_tool(&self, handle: LuaToolHandle) {
        self.tools.lock().unwrap().push(handle);
    }

    /// Register a hook handle (called from bridge).
    pub fn register_hook(&self, handle: LuaHookHandle) {
        self.hooks.lock().unwrap().push(handle);
    }

    /// Register a command handle (called from bridge).
    pub fn register_command(&self, handle: LuaCommandHandle) {
        self.commands.lock().unwrap().push(handle);
    }

    /// Execute a Lua script string.
    pub fn exec(&self, source: &str) -> Result<(), LuaError> {
        self.lua.load(source).exec()?;
        Ok(())
    }

    /// Execute a Lua file.
    pub fn exec_file(&self, path: &std::path::Path) -> Result<(), LuaError> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| LuaError::Extension(format!("{}: {}", path.display(), e)))?;
        self.lua
            .load(&source)
            .set_name(path.to_string_lossy())
            .exec()?;
        Ok(())
    }

    /// Clear all registered tools, hooks, and commands.
    pub fn clear_registrations(&self) {
        self.tools.lock().unwrap().clear();
        self.hooks.lock().unwrap().clear();
        self.commands.lock().unwrap().clear();
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.lock().unwrap().len()
    }

    /// Number of registered hooks.
    pub fn hook_count(&self) -> usize {
        self.hooks.lock().unwrap().len()
    }

    /// Number of registered commands.
    pub fn command_count(&self) -> usize {
        self.commands.lock().unwrap().len()
    }

    /// Get tool names.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    /// Get hook event names.
    pub fn hook_events(&self) -> Vec<String> {
        self.hooks
            .lock()
            .unwrap()
            .iter()
            .map(|h| h.event.clone())
            .collect()
    }

    /// Execute a registered command by name, returning its string output.
    ///
    /// Returns `Ok(None)` if the command returned nil (silent success).
    /// Returns `Ok(Some(text))` if the command returned a string or value.
    /// Returns `Err` if the command handler or name wasn't found.
    pub fn execute_command(&self, name: &str, args: &str) -> Result<Option<String>, LuaError> {
        self.execute_command_with_context(name, args, None)
    }

    /// Execute a registered command with an optional host call context.
    pub fn execute_command_with_context(
        &self,
        name: &str,
        args: &str,
        call_ctx: Option<LuaCallContext>,
    ) -> Result<Option<String>, LuaError> {
        if let Some(ctx) = call_ctx {
            self.set_call_context(ctx);
        }
        let result = self.execute_command_inner(name, args);
        self.clear_call_context();
        result
    }

    fn execute_command_inner(&self, name: &str, args: &str) -> Result<Option<String>, LuaError> {
        let commands = self.commands.lock().unwrap();
        let handle = resolve_command_handle(&commands, name)?;

        let handler: mlua::Function = self
            .lua
            .registry_value(&handle.handler_key)
            .map_err(LuaError::Mlua)?;

        let result: mlua::Value = handler.call(args.to_string()).map_err(LuaError::Mlua)?;

        match result {
            mlua::Value::Nil => Ok(None),
            mlua::Value::String(s) => Ok(Some(
                s.to_str()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|_| "(non-utf8)".into()),
            )),
            other => {
                let json = crate::bridge::lua_value_to_json(other);
                Ok(Some(format!("{json}")))
            }
        }
    }

    /// Get command names.
    pub fn command_names(&self) -> Vec<String> {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .map(LuaCommandHandle::canonical_name)
            .collect()
    }

    /// Get command names with descriptions for menus and discovery.
    pub fn command_summaries(&self) -> Vec<(String, String)> {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .map(|c| (c.canonical_name(), c.description.clone()))
            .collect()
    }

    /// Check if a command with the given name exists.
    pub fn has_command(&self, name: &str) -> bool {
        let commands = self.commands.lock().unwrap();
        resolve_command_handle(&commands, name).is_ok()
    }
}

fn resolve_command_handle<'a>(
    commands: &'a [LuaCommandHandle],
    name: &str,
) -> Result<&'a LuaCommandHandle, LuaError> {
    if let Some(handle) = commands
        .iter()
        .find(|command| command.canonical_name() == name)
    {
        return Ok(handle);
    }

    let mut raw_matches = commands.iter().filter(|command| command.name == name);
    match (raw_matches.next(), raw_matches.next()) {
        (Some(handle), None) => Ok(handle),
        (Some(_), Some(_)) => Err(LuaError::Extension(format!(
            "command '{name}' is ambiguous; use an extension-qualified name"
        ))),
        _ => Err(LuaError::Extension(format!("command '{name}' not found"))),
    }
}
