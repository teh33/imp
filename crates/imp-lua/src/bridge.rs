#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use imp_core::storage;
use imp_core::tools::lua::{parameter_schema_from_lua, tool_output_from_lua_result};
use imp_core::tools::{Tool, ToolContext, ToolOutput, ToolRegistry};
use imp_core::ui::{ComponentSpec, SelectOption};
use imp_core::Error as CoreError;
use imp_llm::auth::AuthStore;
use mlua::{Function, Lua, MultiValue, Table, Value};
use serde_json::{json, Value as JsonValue};
use std::sync::{Arc, Mutex};

use crate::sandbox::{
    LuaCallContext, LuaCommandHandle, LuaError, LuaHookHandle, LuaRuntime, LuaToolHandle,
};

const LUA_EXEC_TIMEOUT: Duration = Duration::from_secs(30);

/// A `Tool` implementation backed by a Lua function registered with
/// `imp.register_tool()`.
pub struct LuaTool {
    name: String,
    label: String,
    description: String,
    readonly: bool,
    params: serde_json::Value,
    runtime: Arc<Mutex<LuaRuntime>>,
    handle_index: usize,
}

#[async_trait]
impl Tool for LuaTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        parameter_schema_from_lua(&self.params)
    }

    fn is_readonly(&self) -> bool {
        self.readonly
    }

    async fn execute(
        &self,
        call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> imp_core::Result<ToolOutput> {
        let runtime = Arc::clone(&self.runtime);
        let handle_index = self.handle_index;
        let call_id = call_id.to_string();
        let ctx_json = json!({
            "cwd": ctx.cwd.display().to_string(),
            "cancelled": ctx.is_cancelled(),
        });
        let call_ctx = LuaCallContext::from(ctx);

        tokio::task::spawn_blocking(move || {
            let runtime_guard = runtime
                .lock()
                .map_err(|_| CoreError::Tool("Lua runtime lock poisoned".into()))?;

            // Make the ToolContext available to imp.tool() during this execution.
            runtime_guard.set_call_context(call_ctx);

            let result = (|| {
                let tools = runtime_guard.tools();
                let handles = tools
                    .lock()
                    .map_err(|_| CoreError::Tool("Lua tool registry lock poisoned".into()))?;
                let handle = handles.get(handle_index).ok_or_else(|| {
                    CoreError::Tool(format!("Lua tool handle {handle_index} not found"))
                })?;

                let execute_fn: Function = runtime_guard
                    .lua()
                    .registry_value(&handle.execute_key)
                    .map_err(lua_tool_error)?;
                let lua_params =
                    json_to_lua_value(runtime_guard.lua(), &params).map_err(lua_tool_error)?;
                let lua_ctx =
                    json_to_lua_value(runtime_guard.lua(), &ctx_json).map_err(lua_tool_error)?;
                let result: Value = execute_fn
                    .call((call_id.as_str(), lua_params, lua_ctx))
                    .map_err(lua_tool_error)?;

                tool_output_from_lua_result(lua_value_to_json(result))
            })();

            runtime_guard.clear_call_context();
            result
        })
        .await
        .map_err(|error| CoreError::Tool(format!("Lua tool task failed: {error}")))?
    }
}

/// Register all currently loaded Lua tools with imp-core's tool registry.
pub fn load_lua_tools(runtime: Arc<Mutex<LuaRuntime>>, registry: &mut ToolRegistry) {
    let handles = {
        let runtime_guard = runtime
            .lock()
            .expect("Lua runtime lock poisoned while loading tools");
        let tools = runtime_guard.tools();
        let handles = tools
            .lock()
            .expect("Lua tool registry lock poisoned while loading tools");

        handles
            .iter()
            .enumerate()
            .map(|(index, handle)| LuaTool {
                name: handle.name.clone(),
                label: handle.label.clone(),
                description: handle.description.clone(),
                readonly: handle.readonly,
                params: handle.params.clone(),
                runtime: Arc::clone(&runtime),
                handle_index: index,
            })
            .collect::<Vec<_>>()
    };

    for tool in handles {
        registry.register(Arc::new(tool));
    }
}

pub(crate) fn run_lua_exec_command(
    mut command: Command,
    timeout: Duration,
) -> std::io::Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn()?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if started.elapsed() >= timeout {
            kill_process_group(&child);
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("imp.exec() timed out after {}s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn kill_process_group(child: &Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        if let Some(pid) = rustix::process::Pid::from_raw(pid) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

fn lua_tool_error(error: mlua::Error) -> CoreError {
    CoreError::Tool(format!("Lua tool error: {error}"))
}

/// Extract header key-value pairs from an optional Lua table.
fn extract_header_pairs(headers: Option<Table>) -> mlua::Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    if let Some(tbl) = headers {
        for pair in tbl.pairs::<String, String>() {
            let (k, v) = pair?;
            pairs.push((k, v));
        }
    }
    Ok(pairs)
}

fn ui_unavailable_result(lua: &Lua) -> mlua::Result<Value> {
    ui_error_result(lua, "unavailable")
}

fn ui_cancelled_result(lua: &Lua) -> mlua::Result<Value> {
    ui_error_result(lua, "cancelled")
}

fn ui_invalid_result(lua: &Lua, message: impl AsRef<str>) -> mlua::Result<Value> {
    let result = lua.create_table()?;
    result.set("ok", false)?;
    result.set("reason", "invalid")?;
    result.set("message", message.as_ref())?;
    Ok(Value::Table(result))
}

fn ui_error_result(lua: &Lua, reason: &str) -> mlua::Result<Value> {
    let result = lua.create_table()?;
    result.set("ok", false)?;
    result.set("reason", reason)?;
    Ok(Value::Table(result))
}

fn ui_ok_result(lua: &Lua, value: JsonValue) -> mlua::Result<Value> {
    let result = lua.create_table()?;
    result.set("ok", true)?;
    result.set("value", json_to_lua_value(lua, &value)?)?;
    Ok(Value::Table(result))
}

fn option_from_json(value: &JsonValue) -> Option<SelectOption> {
    match value {
        JsonValue::String(label) => Some(SelectOption {
            label: label.clone(),
            description: None,
        }),
        JsonValue::Object(object) => Some(SelectOption {
            label: object.get("label")?.as_str()?.to_string(),
            description: object
                .get("description")
                .and_then(JsonValue::as_str)
                .map(str::to_string),
        }),
        _ => None,
    }
}

fn options_from_spec(spec: &JsonValue) -> Result<Vec<SelectOption>, String> {
    let values = spec
        .get("options")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "ui request requires an options array".to_string())?;
    values
        .iter()
        .map(|value| option_from_json(value).ok_or_else(|| "invalid ui option".to_string()))
        .collect()
}

fn component_from_spec(spec: &JsonValue) -> Result<ComponentSpec, String> {
    let component = spec.get("component").unwrap_or(spec);
    serde_json::from_value(component.clone()).map_err(|error| error.to_string())
}

fn execute_ui_request(
    lua: &Lua,
    spec: JsonValue,
    call_ctx: &Arc<Mutex<Option<LuaCallContext>>>,
) -> mlua::Result<Value> {
    let ctx = {
        let ctx_guard = call_ctx
            .lock()
            .map_err(|_| mlua::Error::external("call context lock poisoned"))?;
        match ctx_guard.as_ref() {
            Some(ctx) => ctx.to_tool_context(),
            None => return ui_unavailable_result(lua),
        }
    };

    if !ctx.ui.has_ui() {
        return ui_unavailable_result(lua);
    }

    let kind = spec
        .get("kind")
        .and_then(JsonValue::as_str)
        .unwrap_or("custom");
    let title = spec.get("title").and_then(JsonValue::as_str).unwrap_or("");
    let message = spec
        .get("message")
        .or_else(|| spec.get("context"))
        .and_then(JsonValue::as_str)
        .unwrap_or("");

    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| mlua::Error::external("imp.ui requires a tokio runtime"))?;

    match kind {
        "confirm" => match handle.block_on(ctx.ui.confirm(title, message)) {
            Some(value) => ui_ok_result(lua, JsonValue::Bool(value)),
            None => ui_cancelled_result(lua),
        },
        "select" => {
            let options = match options_from_spec(&spec) {
                Ok(options) => options,
                Err(message) => return ui_invalid_result(lua, message),
            };
            match handle.block_on(ctx.ui.select_with_context(title, message, &options)) {
                Some(index) => ui_ok_result(
                    lua,
                    json!({
                        "index": index + 1,
                        "label": options.get(index).map(|option| option.label.clone()),
                    }),
                ),
                None => ui_cancelled_result(lua),
            }
        }
        "multi_select" | "multi-select" => {
            let options = match options_from_spec(&spec) {
                Ok(options) => options,
                Err(message) => return ui_invalid_result(lua, message),
            };
            match handle.block_on(ctx.ui.multi_select_with_context(title, message, &options)) {
                Some(indices) => {
                    let selected: Vec<JsonValue> = indices
                        .into_iter()
                        .map(|index| {
                            json!({
                                "index": index + 1,
                                "label": options.get(index).map(|option| option.label.clone()),
                            })
                        })
                        .collect();
                    ui_ok_result(lua, JsonValue::Array(selected))
                }
                None => ui_cancelled_result(lua),
            }
        }
        "input" => {
            let placeholder = spec
                .get("placeholder")
                .and_then(JsonValue::as_str)
                .or_else(|| spec.get("default").and_then(JsonValue::as_str))
                .unwrap_or("");
            match handle.block_on(ctx.ui.input_with_context(title, message, placeholder)) {
                Some(value) => ui_ok_result(lua, JsonValue::String(value)),
                None => ui_cancelled_result(lua),
            }
        }
        "custom" => {
            let component = match component_from_spec(&spec) {
                Ok(component) => component,
                Err(message) => return ui_invalid_result(lua, message),
            };
            match handle.block_on(ctx.ui.custom(component)) {
                Some(value) => ui_ok_result(lua, value),
                None => ui_cancelled_result(lua),
            }
        }
        other => ui_invalid_result(lua, format!("unknown ui request kind '{other}'")),
    }
}

/// Set up the `imp` global table with host API functions.
///
/// Exposes to Lua:
/// - imp.on(event, handler)           — subscribe to hook events
/// - imp.register_tool(def)           — register a custom tool
/// - imp.exec(command, args, opts)    — run a shell command
/// - imp.register_command(name, def)  — register a slash command
/// - imp.events.on() / imp.events.emit() — inter-extension event bus
/// - imp.tool(name, params)           — call a native imp tool
/// - imp.ui.request(spec)             — ask the active UI for confirm/select/input/custom
/// - imp.ui.confirm(title, message)   — ergonomic yes/no helper
/// - imp.secret(provider, field?)     — read a saved imp secret field
/// - imp.secret_fields(provider)      — read all saved fields for a provider
/// - imp.env(name)                    — read an env var (scoped by allowed list)
/// - imp.http.get(url, headers?)      — HTTP GET
/// - imp.http.post(url, body, headers?) — HTTP POST
pub fn setup_host_api(runtime: &LuaRuntime) -> Result<(), LuaError> {
    let lua = runtime.lua();

    let imp = lua.create_table()?;

    // ── imp.on(event_name, handler) ──────────────────────────────
    let hooks = runtime.hooks();
    let on_fn = lua.create_function(move |lua_inner, (event, handler): (String, Function)| {
        let key = lua_inner.create_registry_value(handler)?;
        let handle = LuaHookHandle {
            event,
            handler_key: key,
        };
        hooks.lock().unwrap().push(handle);
        Ok(())
    })?;
    imp.set("on", on_fn)?;

    // ── imp.register_tool(definition) ────────────────────────────
    let tools = runtime.tools();
    let register_tool_fn = lua.create_function(move |lua_inner, def: Table| {
        let name: String = def.get("name")?;
        let label: String = def
            .get::<Option<String>>("label")?
            .unwrap_or_else(|| name.clone());
        let description: String = def
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let readonly: bool = def.get::<Option<bool>>("readonly")?.unwrap_or(false);

        let params_val: Value = def.get("params")?;
        let params = lua_value_to_json(params_val);

        let execute_fn: Function = def.get("execute")?;
        let key = lua_inner.create_registry_value(execute_fn)?;

        let handle = LuaToolHandle {
            name,
            label,
            description,
            readonly,
            params,
            execute_key: key,
        };
        tools.lock().unwrap().push(handle);
        Ok(())
    })?;
    imp.set("register_tool", register_tool_fn)?;

    // ── imp.exec(command, args, opts) ────────────────────────────
    let allow_shell_exec = runtime.allow_shell_exec();
    let exec_fn = lua.create_function(
        move |lua_inner, (cmd, args, opts): (String, Option<Table>, Option<Table>)| {
            if !allow_shell_exec.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(mlua::Error::external(
                    "imp.exec() is disabled for this runtime",
                ));
            }
            let output = if let Some(args_table) = args {
                let mut command = Command::new(&cmd);
                for pair in args_table.sequence_values::<String>() {
                    command.arg(pair?);
                }

                if let Some(opts_table) = &opts {
                    if let Ok(Some(cwd)) = opts_table.get::<Option<String>>("cwd") {
                        command.current_dir(cwd);
                    }
                    if let Ok(Some(env_table)) = opts_table.get::<Option<Table>>("env") {
                        for pair in env_table.pairs::<String, String>() {
                            let (name, value) = pair?;
                            command.env(name, value);
                        }
                    }
                }

                command.stdin(Stdio::null());
                run_lua_exec_command(command, LUA_EXEC_TIMEOUT)
            } else {
                let mut command = Command::new("sh");
                command.arg("-c").arg(&cmd);

                if let Some(opts_table) = &opts {
                    if let Ok(Some(cwd)) = opts_table.get::<Option<String>>("cwd") {
                        command.current_dir(cwd);
                    }
                    if let Ok(Some(env_table)) = opts_table.get::<Option<Table>>("env") {
                        for pair in env_table.pairs::<String, String>() {
                            let (name, value) = pair?;
                            command.env(name, value);
                        }
                    }
                }

                command.stdin(Stdio::null());
                run_lua_exec_command(command, LUA_EXEC_TIMEOUT)
            }
            .map_err(mlua::Error::external)?;

            let result = lua_inner.create_table()?;
            result.set(
                "stdout",
                String::from_utf8_lossy(&output.stdout).to_string(),
            )?;
            result.set(
                "stderr",
                String::from_utf8_lossy(&output.stderr).to_string(),
            )?;
            result.set("exit_code", output.status.code().unwrap_or(-1))?;

            Ok(result)
        },
    )?;
    imp.set("exec", exec_fn)?;

    // ── imp.register_command(name, definition) ───────────────────
    let commands = runtime.commands();
    let current_extension = runtime.current_extension();
    let register_command_fn =
        lua.create_function(move |lua_inner, (name, def): (String, Table)| {
            let description: String = def
                .get::<Option<String>>("description")?
                .unwrap_or_default();
            let handler: Function = def
                .get::<Option<Function>>("handler")?
                .or_else(|| def.get::<Option<Function>>("run").ok().flatten())
                .ok_or_else(|| {
                    mlua::Error::external("command definition requires handler or run")
                })?;
            let key = lua_inner.create_registry_value(handler)?;
            let extension = current_extension
                .lock()
                .map_err(|_| mlua::Error::external("current extension lock poisoned"))?
                .clone();

            let handle = LuaCommandHandle {
                name,
                extension,
                description,
                handler_key: key,
            };
            commands.lock().unwrap().push(handle);
            Ok(())
        })?;
    imp.set("register_command", register_command_fn.clone())?;
    imp.set("command", register_command_fn)?;

    // ── imp.events (inter-extension event bus) ───────────────────
    let events = lua.create_table()?;

    // Store handlers in a Lua table: { event_name = { handler1, handler2, ... } }
    let handlers_table = lua.create_table()?;
    lua.set_named_registry_value("__imp_event_handlers", handlers_table)?;

    let events_on = lua.create_function(|lua_inner, (name, handler): (String, Function)| {
        let handlers: Table = lua_inner.named_registry_value("__imp_event_handlers")?;
        let list: Table = match handlers.get::<Option<Table>>(name.as_str())? {
            Some(t) => t,
            None => {
                let t = lua_inner.create_table()?;
                handlers.set(name.as_str(), t.clone())?;
                t
            }
        };
        let len = list.raw_len();
        list.set(len + 1, handler)?;
        Ok(())
    })?;
    events.set("on", events_on)?;

    let events_emit = lua.create_function(|lua_inner, (name, data): (String, Value)| {
        let handlers: Table = lua_inner.named_registry_value("__imp_event_handlers")?;
        if let Some(list) = handlers.get::<Option<Table>>(name.as_str())? {
            for pair in list.sequence_values::<Function>() {
                let handler = pair?;
                // Errors in event handlers are caught and ignored so one bad
                // extension callback cannot destabilize the host runtime.
                let _ = handler.call::<()>(data.clone());
            }
        }
        Ok(())
    })?;
    events.set("emit", events_emit)?;

    imp.set("events", events)?;

    // ── imp.tool(name, params) — call a native imp tool ──────────
    let native_tools = runtime.native_tools();
    let tool_call_ctx = runtime.call_context();
    let allow_native_tool_calls = runtime.allow_native_tool_calls();
    let imp_tool_fn = lua.create_function(
        move |lua_inner, (name, params): (String, Value)| -> mlua::Result<MultiValue> {
            if !allow_native_tool_calls.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(mlua::Error::external(
                    "imp.tool() is disabled for this runtime",
                ));
            }

            // Look up the tool.
            let tool = {
                let tools_guard = native_tools
                    .lock()
                    .map_err(|_| mlua::Error::external("native tools lock poisoned"))?;
                tools_guard
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| mlua::Error::external(format!("tool '{name}' not found")))?
            };

            // Build a ToolContext from the stored call context.
            let ctx = {
                let ctx_guard = tool_call_ctx
                    .lock()
                    .map_err(|_| mlua::Error::external("call context lock poisoned"))?;
                ctx_guard
                    .as_ref()
                    .ok_or_else(|| {
                        mlua::Error::external("imp.tool() called outside of tool execution context")
                    })?
                    .to_tool_context()
            };

            let params_json = lua_value_to_json(params);

            // Execute the tool — async via block_on (safe from spawn_blocking).
            let handle = tokio::runtime::Handle::try_current()
                .map_err(|_| mlua::Error::external("imp.tool() requires a tokio runtime"))?;

            let output = handle
                .block_on(tool.execute("lua-call", params_json, ctx))
                .map_err(|e| mlua::Error::external(format!("tool error: {e}")))?;

            // Convert ToolOutput → Lua multi-return: (result, err).
            let mut mv = MultiValue::new();
            if output.is_error {
                let err_text = output
                    .text_content()
                    .unwrap_or("tool execution failed")
                    .to_string();
                mv.push_back(Value::Nil);
                mv.push_back(Value::String(lua_inner.create_string(&err_text)?));
            } else if let Some(text) = output.text_content() {
                mv.push_back(Value::String(lua_inner.create_string(text)?));
            } else {
                mv.push_back(Value::Nil);
            }
            Ok(mv)
        },
    )?;
    imp.set("tool", imp_tool_fn)?;

    // ── imp.ui — programmatic host interaction ───────────────────
    let ui = lua.create_table()?;
    let ui_call_ctx = runtime.call_context();
    let ui_request_fn = lua.create_function(move |lua_inner, spec: Value| {
        execute_ui_request(lua_inner, lua_value_to_json(spec), &ui_call_ctx)
    })?;
    ui.set("request", ui_request_fn)?;

    let confirm_call_ctx = runtime.call_context();
    let ui_confirm_fn = lua.create_function(
        move |lua_inner, (title, message): (String, Option<String>)| -> mlua::Result<Value> {
            let result = execute_ui_request(
                lua_inner,
                json!({
                    "kind": "confirm",
                    "title": title,
                    "message": message.unwrap_or_default(),
                }),
                &confirm_call_ctx,
            )?;
            let json = lua_value_to_json(result);
            if json.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                Ok(match json.get("value").and_then(JsonValue::as_bool) {
                    Some(value) => Value::Boolean(value),
                    None => Value::Nil,
                })
            } else {
                Ok(Value::Nil)
            }
        },
    )?;
    ui.set("confirm", ui_confirm_fn)?;

    imp.set("ui", ui)?;

    // ── imp.update(text) — stream progress to the TUI ─────────────
    let update_call_ctx = runtime.call_context();
    let imp_update_fn = lua.create_function(move |_lua, text: String| {
        let ctx_guard = update_call_ctx
            .lock()
            .map_err(|_| mlua::Error::external("call context lock poisoned"))?;
        if let Some(ref ctx) = *ctx_guard {
            let _ = ctx.update_tx.try_send(imp_core::tools::ToolUpdate {
                content: vec![imp_core::imp_llm::ContentBlock::Text { text }],
                details: serde_json::Value::Null,
            });
        }
        Ok(())
    })?;
    imp.set("update", imp_update_fn)?;

    // ── imp.secret(provider, field?) — read a saved secret field ──────────
    let allow_secrets = runtime.allow_secrets();
    let secret_fn = lua.create_function(
        move |lua_inner, (provider, field): (String, Option<String>)| -> mlua::Result<Value> {
            if !allow_secrets.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(mlua::Error::external(
                    "imp.secret() is disabled for this runtime",
                ));
            }
            let auth_path =
                storage::existing_global_auth_path().unwrap_or_else(storage::global_auth_path);
            let auth_store =
                AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
            let field = field.unwrap_or_else(|| "api_key".to_string());
            match auth_store.resolve_secret_field(&provider, &field) {
                Ok(value) => Ok(Value::String(lua_inner.create_string(&value)?)),
                Err(error) => Err(mlua::Error::external(error.to_string())),
            }
        },
    )?;
    imp.set("secret", secret_fn)?;

    // ── imp.secret_fields(provider) — read all saved secret fields ─────────
    let allow_secrets = runtime.allow_secrets();
    let secret_fields_fn =
        lua.create_function(move |lua_inner, provider: String| -> mlua::Result<Value> {
            if !allow_secrets.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(mlua::Error::external(
                    "imp.secret_fields() is disabled for this runtime",
                ));
            }
            let auth_path =
                storage::existing_global_auth_path().unwrap_or_else(storage::global_auth_path);
            let auth_store =
                AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
            match auth_store.resolve_secret_fields(&provider) {
                Ok(fields) => {
                    let table = lua_inner.create_table()?;
                    for (field, value) in fields {
                        table.set(field, value)?;
                    }
                    Ok(Value::Table(table))
                }
                Err(error) => Err(mlua::Error::external(error.to_string())),
            }
        })?;
    imp.set("secret_fields", secret_fields_fn)?;

    // ── imp.env(name) — read a scoped env var ────────────────────
    let allowed_env = runtime.allowed_env();
    let env_fn = lua.create_function(move |lua_inner, name: String| {
        let allowed = allowed_env
            .lock()
            .map_err(|_| mlua::Error::external("allowed_env lock poisoned"))?;
        // If the allow-list is empty or the var is not listed, deny access.
        if !allowed.contains(&name) {
            return Ok(Value::Nil);
        }
        match std::env::var(&name) {
            Ok(val) => Ok(Value::String(lua_inner.create_string(&val)?)),
            Err(_) => Ok(Value::Nil),
        }
    })?;
    imp.set("env", env_fn)?;

    // ── imp.http — HTTP GET / POST via reqwest ───────────────────
    let http = lua.create_table()?;
    let allow_http = runtime.allow_http();

    let http_get_fn =
        lua.create_function(move |lua_inner, (url, headers): (String, Option<Table>)| {
            if !allow_http.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(mlua::Error::external(
                    "imp.http.get() is disabled for this runtime",
                ));
            }
            let header_pairs = extract_header_pairs(headers)?;

            let handle = tokio::runtime::Handle::try_current()
                .map_err(|_| mlua::Error::external("imp.http requires a tokio runtime"))?;

            let (status, body) = handle
                .block_on(async {
                    let client = reqwest::Client::new();
                    let mut builder = client.get(&url);
                    for (k, v) in &header_pairs {
                        builder = builder.header(k.as_str(), v.as_str());
                    }
                    let resp = builder.send().await.map_err(|e| e.to_string())?;
                    let status = resp.status().as_u16();
                    let body = resp.text().await.map_err(|e| e.to_string())?;
                    Ok::<_, String>((status, body))
                })
                .map_err(mlua::Error::external)?;

            let result = lua_inner.create_table()?;
            result.set("status", status)?;
            result.set("body", body)?;
            Ok(result)
        })?;
    http.set("get", http_get_fn)?;

    let allow_http = runtime.allow_http();
    let http_post_fn = lua.create_function(
        move |lua_inner, (url, body, headers): (String, String, Option<Table>)| {
            if !allow_http.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(mlua::Error::external(
                    "imp.http.post() is disabled for this runtime",
                ));
            }
            let header_pairs = extract_header_pairs(headers)?;

            let handle = tokio::runtime::Handle::try_current()
                .map_err(|_| mlua::Error::external("imp.http requires a tokio runtime"))?;

            let (status, resp_body) = handle
                .block_on(async {
                    let client = reqwest::Client::new();
                    let mut builder = client.post(&url).body(body);
                    for (k, v) in &header_pairs {
                        builder = builder.header(k.as_str(), v.as_str());
                    }
                    let resp = builder.send().await.map_err(|e| e.to_string())?;
                    let status = resp.status().as_u16();
                    let resp_body = resp.text().await.map_err(|e| e.to_string())?;
                    Ok::<_, String>((status, resp_body))
                })
                .map_err(mlua::Error::external)?;

            let result = lua_inner.create_table()?;
            result.set("status", status)?;
            result.set("body", resp_body)?;
            Ok(result)
        },
    )?;
    http.set("post", http_post_fn)?;

    imp.set("http", http)?;

    // ── Set the global and preload `require("imp")` ──────────────
    let package: Table = lua.globals().get("package")?;
    let loaded: Table = package.get("loaded")?;
    loaded.set("imp", imp.clone())?;
    lua.globals().set("imp", imp)?;

    Ok(())
}

/// Convert a Lua value to serde_json::Value.
pub fn lua_value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(b),
        Value::Integer(i) => serde_json::Value::Number(serde_json::Number::from(i)),
        Value::Number(n) => serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => {
            serde_json::Value::String(s.to_str().map(|s| s.to_string()).unwrap_or_default())
        }
        Value::Table(t) => {
            // Check if it's an array (sequential integer keys starting at 1)
            let len = t.raw_len();
            if len > 0 {
                // Check if all keys 1..=len exist (it's an array)
                let is_array = (1..=len).all(|i| {
                    t.get::<Value>(i)
                        .ok()
                        .map(|v| !matches!(v, Value::Nil))
                        .unwrap_or(false)
                });
                if is_array {
                    let arr: Vec<serde_json::Value> = (1..=len)
                        .filter_map(|i| t.get::<Value>(i).ok().map(lua_value_to_json))
                        .collect();
                    return serde_json::Value::Array(arr);
                }
            }

            // Otherwise it's an object
            let mut map = serde_json::Map::new();
            if let Ok(pairs) = t.pairs::<String, Value>().collect::<Result<Vec<_>, _>>() {
                for (k, v) in pairs {
                    map.insert(k, lua_value_to_json(v));
                }
            }
            serde_json::Value::Object(map)
        }
        _ => serde_json::Value::Null,
    }
}

/// Convert a serde_json::Value to a Lua value.
pub fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    match value {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Number(f))
            } else {
                Ok(Value::Nil)
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let table = lua.create_table()?;
            for (i, v) in arr.iter().enumerate() {
                table.set(i + 1, json_to_lua_value(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table()?;
            for (k, v) in map {
                table.set(k.as_str(), json_to_lua_value(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}
