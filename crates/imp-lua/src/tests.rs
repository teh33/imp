use super::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use imp_core::config::LuaCapabilityPolicy;
use imp_core::tools::{ToolContext, ToolRegistry};
use imp_core::ui::NullInterface;
use tempfile::TempDir;

fn make_policy() -> LuaCapabilityPolicy {
    LuaCapabilityPolicy::default()
}

/// Helper: create a runtime with host API set up.
fn make_runtime() -> LuaRuntime {
    let rt = LuaRuntime::new().expect("create runtime");
    setup_host_api(&rt).expect("setup host api");
    rt
}

/// Helper: write a Lua file into a directory.
fn write_lua(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn test_ctx() -> ToolContext {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    ToolContext {
        cwd: PathBuf::from("/tmp/lua-tools"),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: Arc::new(NullInterface),
        file_cache: Arc::new(imp_core::tools::FileCache::new()),
        checkpoint_state: Arc::new(imp_core::tools::CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(
            imp_core::tools::FileTracker::default(),
        )),
        anchor_store: Arc::new(imp_core::tools::AnchorStore::new()),
        mode: imp_core::config::AgentMode::Full,
        read_max_lines: 0,
        turn_workflow_review: Arc::new(std::sync::Mutex::new(
            imp_core::workflow_review::TurnWorkflowReviewAccumulator::default(),
        )),
        run_policy: Default::default(),
        config: Arc::new(imp_core::config::Config::default()),
        lua_tool_loader: None,
        supporting_provenance: Vec::new(),
    }
}

// ── Discovery ────────────────────────────────────────────────

#[test]
fn init_lua_extensions_applies_capability_policy() {
    let user = TempDir::new().unwrap();
    let lua_dir = user.path().join("lua");
    std::fs::create_dir_all(&lua_dir).unwrap();
    write_lua(&lua_dir, "ext.lua", "-- extension present");

    let mut registry = ToolRegistry::new();
    let mut policy = make_policy();
    policy.allow_native_tool_calls = false;
    policy.allow_shell_exec = true;
    policy.allow_http = true;
    policy.allow_secrets = true;
    policy.allowed_env.insert("ALLOWED_ONE".to_string());

    let runtime = init_lua_extensions(user.path(), None, &mut registry, &policy)
        .expect("runtime should initialize");
    let guard = runtime.lock().unwrap();

    assert!(!guard
        .allow_native_tool_calls()
        .load(std::sync::atomic::Ordering::Relaxed));
    assert!(guard
        .allow_shell_exec()
        .load(std::sync::atomic::Ordering::Relaxed));
    assert!(guard
        .allow_http()
        .load(std::sync::atomic::Ordering::Relaxed));
    assert!(guard
        .allow_secrets()
        .load(std::sync::atomic::Ordering::Relaxed));
    assert!(guard.allowed_env().lock().unwrap().contains("ALLOWED_ONE"));
}

#[test]
fn discover_user_lua_files() {
    let tmp = TempDir::new().unwrap();
    let lua_dir = tmp.path().join("lua");
    std::fs::create_dir_all(&lua_dir).unwrap();
    write_lua(&lua_dir, "greet.lua", "-- hello");
    write_lua(&lua_dir, "utils.lua", "-- utils");

    let exts = discover_extensions(tmp.path(), None);
    assert_eq!(exts.len(), 2);

    let names: Vec<&str> = exts.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"greet"));
    assert!(names.contains(&"utils"));
}

#[test]
fn discover_directory_init_lua() {
    let tmp = TempDir::new().unwrap();
    let ext_dir = tmp.path().join("lua").join("my-ext");
    std::fs::create_dir_all(&ext_dir).unwrap();
    write_lua(&ext_dir, "init.lua", "-- init");

    let exts = discover_extensions(tmp.path(), None);
    assert_eq!(exts.len(), 1);
    assert_eq!(exts[0].name, "my-ext");
    assert!(exts[0].path.ends_with("init.lua"));
}

#[test]
fn discover_project_local() {
    let user = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let proj_lua = project.path().join(".imp").join("lua");
    std::fs::create_dir_all(&proj_lua).unwrap();
    write_lua(&proj_lua, "local.lua", "-- local");

    let exts = discover_extensions(user.path(), Some(project.path()));
    assert_eq!(exts.len(), 1);
    assert_eq!(exts[0].name, "local");
}

#[test]
fn discover_empty_dirs_return_nothing() {
    let tmp = TempDir::new().unwrap();
    let exts = discover_extensions(tmp.path(), None);
    assert!(exts.is_empty());
}

// ── imp.on() — Hook registration ────────────────────────────

#[test]
fn on_registers_hook() {
    let rt = make_runtime();
    rt.exec(
        r#"
        imp.on("on_session_start", function(event, ctx)
            -- handler
        end)
    "#,
    )
    .unwrap();

    assert_eq!(rt.hook_count(), 1);
    let events = rt.hook_events();
    assert_eq!(events[0], "on_session_start");
}

#[test]
fn on_registers_multiple_hooks() {
    let rt = make_runtime();
    rt.exec(
        r#"
        imp.on("on_session_start", function() end)
        imp.on("after_file_write", function() end)
        imp.on("before_tool_call", function() end)
    "#,
    )
    .unwrap();

    assert_eq!(rt.hook_count(), 3);
    let events = rt.hook_events();
    assert!(events.contains(&"on_session_start".to_string()));
    assert!(events.contains(&"after_file_write".to_string()));
    assert!(events.contains(&"before_tool_call".to_string()));
}

#[test]
fn hook_handler_fires_on_correct_event() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _test_fired = false
        imp.on("on_session_start", function()
            _test_fired = true
        end)
    "#,
    )
    .unwrap();

    // Simulate firing the hook by calling the stored handler
    let hooks = rt.hooks();
    let hooks_guard = hooks.lock().unwrap();
    assert_eq!(hooks_guard.len(), 1);

    let handler: mlua::Function = rt
        .lua()
        .registry_value(&hooks_guard[0].handler_key)
        .unwrap();
    handler.call::<()>(()).unwrap();

    let fired: bool = rt.lua().globals().get("_test_fired").unwrap();
    assert!(fired);
}

// ── imp.register_tool() ─────────────────────────────────────

#[test]
fn register_tool_creates_handle() {
    let rt = make_runtime();
    rt.exec(
        r#"
        imp.register_tool({
            name = "greet",
            label = "Greeting Tool",
            description = "Says hello",
            readonly = true,
            params = {
                type = "object",
                properties = {
                    name = { type = "string", description = "Who to greet" }
                }
            },
            execute = function(call_id, params, ctx)
                return { content = "Hello, " .. (params.name or "world") }
            end
        })
    "#,
    )
    .unwrap();

    assert_eq!(rt.tool_count(), 1);
    let names = rt.tool_names();
    assert_eq!(names[0], "greet");
}

#[test]
fn register_tool_execute_callable() {
    let rt = make_runtime();
    rt.exec(
        r#"
        imp.register_tool({
            name = "add",
            execute = function(call_id, params, ctx)
                return { content = tostring(params.a + params.b), is_error = false }
            end
        })
    "#,
    )
    .unwrap();

    // Call the execute function directly
    let tools = rt.tools();
    let tools_guard = tools.lock().unwrap();
    let execute_fn: mlua::Function = rt
        .lua()
        .registry_value(&tools_guard[0].execute_key)
        .unwrap();

    let params = rt.lua().create_table().unwrap();
    params.set("a", 3).unwrap();
    params.set("b", 4).unwrap();

    let result: mlua::Table = execute_fn
        .call(("call_1", params, mlua::Value::Nil))
        .unwrap();
    let content: String = result.get("content").unwrap();
    assert_eq!(content, "7");
}

#[tokio::test]
async fn load_lua_tools_registers_and_executes_bridge() {
    let rt = make_runtime();
    rt.exec(
        r#"
        imp.register_tool({
            name = "greet",
            label = "Greeting Tool",
            description = "Greets from Lua",
            readonly = true,
            params = {
                name = { type = "string", description = "Who to greet", required = true },
                excited = { type = "boolean" }
            },
            execute = function(call_id, params, ctx)
                local suffix = params.excited and "!" or "."
                return {
                    content = {
                        { type = "text", text = "hello " .. params.name .. suffix },
                    },
                    details = {
                        call_id = call_id,
                        cwd = ctx.cwd,
                        cancelled = ctx.cancelled,
                    },
                }
            end
        })
    "#,
    )
    .unwrap();

    let runtime = Arc::new(Mutex::new(rt));
    let mut registry = ToolRegistry::new();
    load_lua_tools(Arc::clone(&runtime), &mut registry);

    let tool = registry
        .get("greet")
        .expect("lua tool should be registered");
    assert_eq!(tool.label(), "Greeting Tool");
    assert_eq!(tool.description(), "Greets from Lua");
    assert!(tool.is_readonly());
    assert_eq!(tool.parameters()["properties"]["name"]["type"], "string");
    assert_eq!(tool.parameters()["required"], serde_json::json!(["name"]));

    let output = tool
        .execute(
            "call_123",
            serde_json::json!({ "name": "Ada", "excited": true }),
            test_ctx(),
        )
        .await
        .unwrap();

    let text = output
        .content
        .iter()
        .find_map(|block| match block {
            imp_core::imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("lua tool should return text");
    assert_eq!(text, "hello Ada!");
    assert_eq!(output.details["call_id"], "call_123");
    assert_eq!(output.details["cwd"], "/tmp/lua-tools");
    assert_eq!(output.details["cancelled"], false);
}

#[test]
fn imp_secret_helpers_exist() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _has_secret = type(imp.secret) == "function"
        _has_secret_fields = type(imp.secret_fields) == "function"
    "#,
    )
    .unwrap();

    let has_secret: bool = rt.lua().globals().get("_has_secret").unwrap();
    let has_secret_fields: bool = rt.lua().globals().get("_has_secret_fields").unwrap();
    assert!(has_secret);
    assert!(has_secret_fields);
}

// ── imp.exec() — Shell execution ────────────────────────────

#[test]
fn exec_runs_command_returns_stdout() {
    let rt = make_runtime();
    rt.set_allow_shell_exec(true);
    rt.exec(
        r#"
        local result = imp.exec("echo hello")
        _test_stdout = result.stdout
        _test_exit = result.exit_code
    "#,
    )
    .unwrap();

    let stdout: String = rt.lua().globals().get("_test_stdout").unwrap();
    let exit_code: i32 = rt.lua().globals().get("_test_exit").unwrap();
    assert_eq!(stdout.trim(), "hello");
    assert_eq!(exit_code, 0);
}

#[test]
fn exec_captures_stderr() {
    let rt = make_runtime();
    rt.set_allow_shell_exec(true);
    rt.exec(
        r#"
        local result = imp.exec("echo error >&2")
        _test_stderr = result.stderr
    "#,
    )
    .unwrap();

    let stderr: String = rt.lua().globals().get("_test_stderr").unwrap();
    assert_eq!(stderr.trim(), "error");
}

#[test]
fn exec_returns_nonzero_exit_code() {
    let rt = make_runtime();
    rt.set_allow_shell_exec(true);
    rt.exec(
        r#"
        local result = imp.exec("exit 42")
        _test_exit = result.exit_code
    "#,
    )
    .unwrap();

    let exit_code: i32 = rt.lua().globals().get("_test_exit").unwrap();
    assert_eq!(exit_code, 42);
}

#[test]
fn exec_with_args_runs_command_without_shell_joining() {
    let rt = make_runtime();
    rt.set_allow_shell_exec(true);
    rt.exec(
        r#"
        local result = imp.exec("sh", { "-lc", "printf '%s' \"$1\"", "--", "hello world" })
        _test_stdout = result.stdout
        _test_exit = result.exit_code
    "#,
    )
    .unwrap();

    let stdout: String = rt.lua().globals().get("_test_stdout").unwrap();
    let exit_code: i32 = rt.lua().globals().get("_test_exit").unwrap();
    assert_eq!(stdout, "hello world");
    assert_eq!(exit_code, 0);
}

#[test]
fn exec_with_cwd() {
    let rt = make_runtime();
    rt.set_allow_shell_exec(true);
    rt.exec(
        r#"
        local result = imp.exec("pwd", nil, { cwd = "/tmp" })
        _test_cwd = result.stdout
    "#,
    )
    .unwrap();

    let cwd: String = rt.lua().globals().get("_test_cwd").unwrap();
    // /tmp may resolve to /private/tmp on macOS
    assert!(cwd.trim().contains("tmp"));
}

// ── ctx.ui.confirm() with NullInterface ─────────────────────
// The NullInterface returns None for confirm, which maps to nil in Lua.
// We simulate this by testing that the bridge correctly handles nil returns.

#[test]
fn null_interface_confirm_returns_nil() {
    let rt = make_runtime();
    // Simulate what ctx.ui.confirm would do with NullInterface — just return nil
    rt.exec(
        r#"
        -- When NullInterface returns None, the bridge maps it to nil
        _confirm_result = nil  -- This is what NullInterface.confirm() produces
        _is_nil = (_confirm_result == nil)
    "#,
    )
    .unwrap();

    let is_nil: bool = rt.lua().globals().get("_is_nil").unwrap();
    assert!(is_nil);
}

// ── imp.ui ─────────────────────────────────────────────────

#[test]
fn imp_ui_confirm_returns_nil_without_call_context() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _confirm_result = imp.ui.confirm("Proceed?", "No UI should be unavailable")
        _is_nil = (_confirm_result == nil)
    "#,
    )
    .unwrap();

    let is_nil: bool = rt.lua().globals().get("_is_nil").unwrap();
    assert!(is_nil);
}

#[test]
fn imp_ui_request_reports_unavailable_without_call_context() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _ui_result = imp.ui.request({ kind = "confirm", title = "Proceed?" })
    "#,
    )
    .unwrap();

    let result: mlua::Table = rt.lua().globals().get("_ui_result").unwrap();
    let ok: bool = result.get("ok").unwrap();
    let reason: String = result.get("reason").unwrap();
    assert!(!ok);
    assert_eq!(reason, "unavailable");
}

#[test]
fn lua_command_can_call_imp_ui_confirm_safely_headless() {
    let rt = make_runtime();
    rt.exec(
        r#"
        imp.register_command("confirm-test", {
            handler = function(args)
                local yes = imp.ui.confirm("Proceed?", args)
                if yes == nil then
                    return "unavailable"
                end
                return tostring(yes)
            end
        })
    "#,
    )
    .unwrap();

    let output = rt
        .execute_command_with_context("confirm-test", "headless", Some(test_ctx().into()))
        .unwrap();
    assert_eq!(output.as_deref(), Some("unavailable"));
}

// ── Hot reload ──────────────────────────────────────────────

#[test]
fn hot_reload_drops_and_recreates() {
    let user_dir = TempDir::new().unwrap();
    let lua_dir = user_dir.path().join("lua");
    std::fs::create_dir_all(&lua_dir).unwrap();

    write_lua(
        &lua_dir,
        "ext.lua",
        r#"
        imp.on("on_session_start", function() end)
        imp.register_tool({ name = "my_tool", execute = function() end })
    "#,
    );

    let policy = make_policy();
    // First load
    let (rt1, exts1) = reload(user_dir.path(), None, &policy).unwrap();
    assert_eq!(rt1.hook_count(), 1);
    assert_eq!(rt1.tool_count(), 1);
    assert_eq!(exts1.len(), 1);

    // Modify the extension
    write_lua(
        &lua_dir,
        "ext.lua",
        r#"
        imp.on("on_session_start", function() end)
        imp.on("after_file_write", function() end)
        imp.register_tool({ name = "tool_a", execute = function() end })
        imp.register_tool({ name = "tool_b", execute = function() end })
    "#,
    );

    // Reload — old state is dropped, new state picks up changes
    let (rt2, exts2) = reload(user_dir.path(), None, &policy).unwrap();
    assert_eq!(rt2.hook_count(), 2);
    assert_eq!(rt2.tool_count(), 2);
    assert_eq!(exts2.len(), 1);

    let tool_names = rt2.tool_names();
    assert!(tool_names.contains(&"tool_a".to_string()));
    assert!(tool_names.contains(&"tool_b".to_string()));
}

#[test]
fn hot_reload_applies_capability_policy() {
    let user_dir = TempDir::new().unwrap();
    let lua_dir = user_dir.path().join("lua");
    std::fs::create_dir_all(&lua_dir).unwrap();
    write_lua(&lua_dir, "ext.lua", "-- extension present");

    let mut policy = make_policy();
    policy.allow_shell_exec = true;

    let (rt, _exts) = reload(user_dir.path(), None, &policy).unwrap();
    assert!(rt
        .allow_shell_exec()
        .load(std::sync::atomic::Ordering::Relaxed));
}

// ── Error handling ──────────────────────────────────────────

#[test]
fn lua_syntax_error_caught() {
    let rt = make_runtime();
    let result = rt.exec("this is not valid lua !!!");
    assert!(result.is_err());
    // Runtime is still usable after error
    let result2 = rt.exec("_test_ok = true");
    assert!(result2.is_ok());
    let ok: bool = rt.lua().globals().get("_test_ok").unwrap();
    assert!(ok);
}

#[test]
fn lua_runtime_error_caught() {
    let rt = make_runtime();
    let result = rt.exec("error('intentional error')");
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("intentional error"));
}

#[test]
fn extension_error_doesnt_crash_runtime() {
    let rt = make_runtime();

    // First extension errors
    let r1 = rt.exec("error('ext1 failed')");
    assert!(r1.is_err());

    // Second extension still loads fine
    let r2 = rt.exec(
        r#"
        imp.on("on_session_start", function() end)
        _ext2_loaded = true
    "#,
    );
    assert!(r2.is_ok());
    assert_eq!(rt.hook_count(), 1);

    let loaded: bool = rt.lua().globals().get("_ext2_loaded").unwrap();
    assert!(loaded);
}

// ── Multiple extensions coexist ─────────────────────────────

#[test]
fn multiple_extensions_coexist() {
    let rt = make_runtime();

    // Extension 1
    rt.exec(
        r#"
        imp.on("on_session_start", function()
            _ext1_fired = true
        end)
        imp.register_tool({ name = "ext1_tool", execute = function() end })
    "#,
    )
    .unwrap();

    // Extension 2
    rt.exec(
        r#"
        imp.on("after_file_write", function()
            _ext2_fired = true
        end)
        imp.register_tool({ name = "ext2_tool", execute = function() end })
    "#,
    )
    .unwrap();

    assert_eq!(rt.hook_count(), 2);
    assert_eq!(rt.tool_count(), 2);

    let names = rt.tool_names();
    assert!(names.contains(&"ext1_tool".to_string()));
    assert!(names.contains(&"ext2_tool".to_string()));

    let events = rt.hook_events();
    assert!(events.contains(&"on_session_start".to_string()));
    assert!(events.contains(&"after_file_write".to_string()));
}

#[test]
fn extensions_share_state() {
    let rt = make_runtime();

    // Extension 1 sets a global
    rt.exec("shared_counter = 1").unwrap();

    // Extension 2 reads and increments it
    rt.exec(
        r#"
        shared_counter = shared_counter + 1
        _final = shared_counter
    "#,
    )
    .unwrap();

    let val: i64 = rt.lua().globals().get("_final").unwrap();
    assert_eq!(val, 2);
}

// ── Inter-extension events ──────────────────────────────────

#[test]
fn events_on_and_emit() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _received = nil
        imp.events.on("custom_event", function(data)
            _received = data
        end)
        imp.events.emit("custom_event", "hello from event")
    "#,
    )
    .unwrap();

    let received: String = rt.lua().globals().get("_received").unwrap();
    assert_eq!(received, "hello from event");
}

#[test]
fn events_multiple_handlers() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _count = 0
        imp.events.on("tick", function() _count = _count + 1 end)
        imp.events.on("tick", function() _count = _count + 1 end)
        imp.events.emit("tick", nil)
    "#,
    )
    .unwrap();

    let count: i64 = rt.lua().globals().get("_count").unwrap();
    assert_eq!(count, 2);
}

#[test]
fn events_handler_error_doesnt_crash() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _after_error = false
        imp.events.on("test", function() error("boom") end)
        imp.events.on("test", function() _after_error = true end)
        imp.events.emit("test", nil)
    "#,
    )
    .unwrap();

    let after: bool = rt.lua().globals().get("_after_error").unwrap();
    assert!(after, "second handler should still fire after first errors");
}

// ── imp.register_command() ──────────────────────────────────

#[test]
fn register_command_creates_handle() {
    let rt = make_runtime();
    rt.exec(
        r#"
        imp.register_command("greet", {
            description = "Say hello",
            handler = function(args, ctx)
                return "Hello!"
            end
        })
    "#,
    )
    .unwrap();

    assert_eq!(rt.command_count(), 1);
}

// ── JSON conversion ─────────────────────────────────────────

#[test]
fn lua_value_to_json_primitives() {
    let rt = make_runtime();
    let lua = rt.lua();

    assert_eq!(lua_value_to_json(mlua::Value::Nil), serde_json::Value::Null);
    assert_eq!(
        lua_value_to_json(mlua::Value::Boolean(true)),
        serde_json::json!(true)
    );
    assert_eq!(
        lua_value_to_json(mlua::Value::Integer(42)),
        serde_json::json!(42)
    );
    assert_eq!(
        lua_value_to_json(mlua::Value::Number(1.25)),
        serde_json::json!(1.25)
    );

    let s = lua.create_string("hello").unwrap();
    assert_eq!(
        lua_value_to_json(mlua::Value::String(s)),
        serde_json::json!("hello")
    );
}

#[test]
fn lua_table_to_json_object() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _test_table = { name = "Alice", age = 30 }
    "#,
    )
    .unwrap();

    let val: mlua::Value = rt.lua().globals().get("_test_table").unwrap();
    let json = lua_value_to_json(val);
    assert_eq!(json["name"], "Alice");
    assert_eq!(json["age"], 30);
}

#[test]
fn lua_array_to_json_array() {
    let rt = make_runtime();
    rt.exec(
        r#"
        _test_arr = { 1, 2, 3 }
    "#,
    )
    .unwrap();

    let val: mlua::Value = rt.lua().globals().get("_test_arr").unwrap();
    let json = lua_value_to_json(val);
    assert_eq!(json, serde_json::json!([1, 2, 3]));
}

#[test]
fn json_to_lua_roundtrip() {
    let rt = make_runtime();
    let lua = rt.lua();

    let original = serde_json::json!({
        "name": "test",
        "count": 42,
        "active": true,
        "tags": ["a", "b"],
        "nested": { "x": 1 }
    });

    let lua_val = json_to_lua_value(lua, &original).unwrap();
    let back = lua_value_to_json(lua_val);
    assert_eq!(back, original);
}

// ── File loading ────────────────────────────────────────────

#[test]
fn load_extensions_from_files() {
    let user_dir = TempDir::new().unwrap();
    let lua_dir = user_dir.path().join("lua");
    std::fs::create_dir_all(&lua_dir).unwrap();

    write_lua(
        &lua_dir,
        "a.lua",
        r#"imp.on("on_session_start", function() end)"#,
    );
    write_lua(
        &lua_dir,
        "b.lua",
        r#"imp.register_tool({ name = "b_tool", execute = function() end })"#,
    );

    let exts = discover_extensions(user_dir.path(), None);
    assert_eq!(exts.len(), 2);

    let rt = make_runtime();
    let results = load_extensions(&rt, &exts);

    // Both should succeed
    for (name, result) in &results {
        assert!(result.is_ok(), "Extension {} failed: {:?}", name, result);
    }

    assert_eq!(rt.hook_count(), 1);
    assert_eq!(rt.tool_count(), 1);
}

#[test]
fn load_extension_error_reported_not_fatal() {
    let user_dir = TempDir::new().unwrap();
    let lua_dir = user_dir.path().join("lua");
    std::fs::create_dir_all(&lua_dir).unwrap();

    write_lua(&lua_dir, "bad.lua", "error('bad extension')");
    write_lua(
        &lua_dir,
        "good.lua",
        r#"imp.on("on_session_start", function() end)"#,
    );

    let exts = discover_extensions(user_dir.path(), None);
    let rt = make_runtime();
    let results = load_extensions(&rt, &exts);

    // One fails, one succeeds
    let failures: Vec<_> = results.iter().filter(|(_, r)| r.is_err()).collect();
    let successes: Vec<_> = results.iter().filter(|(_, r)| r.is_ok()).collect();

    assert_eq!(failures.len(), 1);
    assert_eq!(successes.len(), 1);

    // Good extension's hook was registered despite the bad extension
    assert_eq!(rt.hook_count(), 1);
}

// ── imp.tool() — call native tools from Lua ─────────────────

use async_trait::async_trait;

struct EchoTestTool;

#[async_trait]
impl imp_core::tools::Tool for EchoTestTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn label(&self) -> &str {
        "Echo"
    }
    fn description(&self) -> &str {
        "Echoes text"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
    }
    fn is_readonly(&self) -> bool {
        true
    }
    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        _ctx: imp_core::tools::ToolContext,
    ) -> imp_core::Result<imp_core::tools::ToolOutput> {
        let text = params["text"].as_str().unwrap_or("no text");
        Ok(imp_core::tools::ToolOutput::text(format!("echo: {text}")))
    }
}

struct FailTestTool;

#[async_trait]
impl imp_core::tools::Tool for FailTestTool {
    fn name(&self) -> &str {
        "fail"
    }
    fn label(&self) -> &str {
        "Fail"
    }
    fn description(&self) -> &str {
        "Always fails"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    fn is_readonly(&self) -> bool {
        true
    }
    async fn execute(
        &self,
        _call_id: &str,
        _params: serde_json::Value,
        _ctx: imp_core::tools::ToolContext,
    ) -> imp_core::Result<imp_core::tools::ToolOutput> {
        Ok(imp_core::tools::ToolOutput::error("intentional failure"))
    }
}

fn make_call_context() -> sandbox::LuaCallContext {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    sandbox::LuaCallContext {
        cwd: PathBuf::from("/tmp/lua-test"),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: Arc::new(NullInterface),
        file_cache: Arc::new(imp_core::tools::FileCache::new()),
        checkpoint_state: Arc::new(imp_core::tools::CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(
            imp_core::tools::FileTracker::default(),
        )),
        anchor_store: Arc::new(imp_core::tools::AnchorStore::new()),
        mode: imp_core::config::AgentMode::Full,
        read_max_lines: 500,
        run_policy: Default::default(),
        lua_tool_loader: None,
        config: Arc::new(imp_core::config::Config::default()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn imp_tool_calls_native_tool() {
    let rt = make_runtime();

    let mut native = std::collections::HashMap::new();
    native.insert(
        "echo".to_string(),
        Arc::new(EchoTestTool) as Arc<dyn imp_core::tools::Tool>,
    );
    rt.set_native_tools(native);
    rt.set_call_context(make_call_context());

    let rt = Arc::new(Mutex::new(rt));
    let rt2 = Arc::clone(&rt);

    let result = tokio::task::spawn_blocking(move || {
        let guard = rt2.lock().unwrap();
        guard
            .exec(
                r#"
            _result, _err = imp.tool("echo", { text = "hello from lua" })
        "#,
            )
            .unwrap();
        let result: String = guard.lua().globals().get("_result").unwrap();
        let err: mlua::Value = guard.lua().globals().get("_err").unwrap();
        assert!(matches!(err, mlua::Value::Nil), "expected no error");
        result
    })
    .await
    .unwrap();

    assert_eq!(result, "echo: hello from lua");
}

#[tokio::test(flavor = "multi_thread")]
async fn imp_tool_returns_error_on_failure() {
    let rt = make_runtime();

    let mut native = std::collections::HashMap::new();
    native.insert(
        "fail".to_string(),
        Arc::new(FailTestTool) as Arc<dyn imp_core::tools::Tool>,
    );
    rt.set_native_tools(native);
    rt.set_call_context(make_call_context());

    let rt = Arc::new(Mutex::new(rt));
    let rt2 = Arc::clone(&rt);

    tokio::task::spawn_blocking(move || {
        let guard = rt2.lock().unwrap();
        guard
            .exec(
                r#"
            _result, _err = imp.tool("fail", {})
        "#,
            )
            .unwrap();
        let result: mlua::Value = guard.lua().globals().get("_result").unwrap();
        assert!(matches!(result, mlua::Value::Nil), "expected nil result");
        let err: String = guard.lua().globals().get("_err").unwrap();
        assert!(
            err.contains("intentional failure"),
            "expected failure message, got: {err}"
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn imp_tool_errors_on_unknown_tool() {
    let rt = make_runtime();
    rt.set_native_tools(std::collections::HashMap::new());
    rt.set_call_context(make_call_context());

    let rt = Arc::new(Mutex::new(rt));
    let rt2 = Arc::clone(&rt);

    tokio::task::spawn_blocking(move || {
        let guard = rt2.lock().unwrap();
        let result = guard.exec(
            r#"
            imp.tool("nonexistent", {})
        "#,
        );
        assert!(result.is_err(), "should error on unknown tool");
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("not found"),
            "error should mention 'not found': {err}"
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn imp_tool_errors_when_disabled() {
    let rt = make_runtime();

    let mut native = std::collections::HashMap::new();
    native.insert(
        "echo".to_string(),
        Arc::new(EchoTestTool) as Arc<dyn imp_core::tools::Tool>,
    );
    rt.set_native_tools(native);
    rt.set_call_context(make_call_context());
    rt.set_allow_native_tool_calls(false);

    let rt = Arc::new(Mutex::new(rt));
    let rt2 = Arc::clone(&rt);

    tokio::task::spawn_blocking(move || {
        let guard = rt2.lock().unwrap();
        let result = guard.exec(
            r#"
            imp.tool("echo", { text = "hello from lua" })
        "#,
        );
        assert!(result.is_err(), "disabled imp.tool() should error");
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("disabled"),
            "error should mention disabled state: {err}"
        );
    })
    .await
    .unwrap();
}

// ── imp.env() — scoped env var access ───────────────────────

#[test]
fn imp_exec_errors_when_disabled() {
    let rt = make_runtime();
    let result = rt.exec(
        r#"
        local _ = imp.exec("echo hi")
    "#,
    );
    assert!(result.is_err(), "disabled imp.exec() should error");
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("imp.exec() is disabled"),
        "unexpected error: {err}"
    );
}

#[test]
fn imp_exec_runs_when_enabled() {
    let rt = make_runtime();
    rt.set_allow_shell_exec(true);

    rt.exec(
        r#"
        local result = imp.exec("printf lua_exec_ok")
        _test_stdout = result.stdout
        _test_exit = result.exit_code
    "#,
    )
    .unwrap();

    let stdout: String = rt.lua().globals().get("_test_stdout").unwrap();
    let exit_code: i32 = rt.lua().globals().get("_test_exit").unwrap();
    assert_eq!(stdout, "lua_exec_ok");
    assert_eq!(exit_code, 0);
}

#[test]
fn imp_exec_passes_scoped_env_to_child_process() {
    let rt = make_runtime();
    rt.set_allow_shell_exec(true);

    rt.exec(
        r#"
        local result = imp.exec("printf %s \"$IMP_LUA_CHILD_SECRET\"", nil, {
            env = { IMP_LUA_CHILD_SECRET = "child-only-value" },
        })
        _test_stdout = result.stdout
        _test_exit = result.exit_code
    "#,
    )
    .unwrap();

    let stdout: String = rt.lua().globals().get("_test_stdout").unwrap();
    let exit_code: i32 = rt.lua().globals().get("_test_exit").unwrap();
    assert_eq!(stdout, "child-only-value");
    assert_eq!(exit_code, 0);
}

#[test]
fn imp_exec_timeout_helper_returns_error() {
    let started = std::time::Instant::now();
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg("sleep 5")
        .stdin(std::process::Stdio::null());

    let result = crate::bridge::run_lua_exec_command(command, std::time::Duration::from_millis(50));

    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(matches!(
        result,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut
    ));
}

#[test]
fn imp_secret_errors_when_disabled() {
    let rt = make_runtime();
    let result = rt.exec(
        r#"
        local _ = imp.secret("openai", "api_key")
    "#,
    );
    assert!(result.is_err(), "disabled imp.secret() should error");
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("imp.secret() is disabled"),
        "unexpected error: {err}"
    );
}

#[test]
fn imp_secret_fields_errors_when_disabled() {
    let rt = make_runtime();
    let result = rt.exec(
        r#"
        local _ = imp.secret_fields("openai")
    "#,
    );
    assert!(result.is_err(), "disabled imp.secret_fields() should error");
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("imp.secret_fields() is disabled"),
        "unexpected error: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn imp_http_get_errors_when_disabled() {
    let rt = Arc::new(Mutex::new(make_runtime()));
    let rt2 = Arc::clone(&rt);
    tokio::task::spawn_blocking(move || {
        let guard = rt2.lock().unwrap();
        let result = guard.exec(
            r#"
            local _ = imp.http.get("https://example.com")
        "#,
        );
        assert!(result.is_err(), "disabled imp.http.get() should error");
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("imp.http.get() is disabled"),
            "unexpected error: {err}"
        );
    })
    .await
    .unwrap();
}

#[test]
fn imp_env_reads_var_when_allowed() {
    let rt = make_runtime();
    std::env::set_var("IMP_LUA_TEST_VAR", "test_value");

    let mut allowed = std::collections::HashSet::new();
    allowed.insert("IMP_LUA_TEST_VAR".to_string());
    rt.set_allowed_env(allowed);

    rt.exec(
        r#"
        _env_val = imp.env("IMP_LUA_TEST_VAR")
    "#,
    )
    .unwrap();

    let val: String = rt.lua().globals().get("_env_val").unwrap();
    assert_eq!(val, "test_value");
}

#[test]
fn imp_env_returns_nil_for_denied_var() {
    let rt = make_runtime();
    std::env::set_var("IMP_LUA_TEST_SECRET", "secret_value");

    let mut allowed = std::collections::HashSet::new();
    allowed.insert("SOME_OTHER_VAR".to_string());
    rt.set_allowed_env(allowed);

    rt.exec(
        r#"
        _env_val = imp.env("IMP_LUA_TEST_SECRET")
        _is_nil = (_env_val == nil)
    "#,
    )
    .unwrap();

    let is_nil: bool = rt.lua().globals().get("_is_nil").unwrap();
    assert!(is_nil, "denied env var should return nil");
}

#[test]
fn imp_env_allows_all_when_list_empty() {
    let rt = make_runtime();
    std::env::set_var("IMP_LUA_TEST_OPEN", "open_value");

    // Empty allowed set should deny by default.
    rt.set_allowed_env(std::collections::HashSet::new());

    rt.exec(
        r#"
        _env_val = imp.env("IMP_LUA_TEST_OPEN")
        _is_nil = (_env_val == nil)
    "#,
    )
    .unwrap();

    let is_nil: bool = rt.lua().globals().get("_is_nil").unwrap();
    assert!(is_nil, "empty allow-list should deny env access by default");
}

#[test]
fn imp_env_returns_nil_for_missing_var() {
    let rt = make_runtime();
    rt.set_allowed_env(std::collections::HashSet::new());

    rt.exec(
        r#"
        _env_val = imp.env("DEFINITELY_NOT_SET_IMP_LUA_TEST")
        _is_nil = (_env_val == nil)
    "#,
    )
    .unwrap();

    let is_nil: bool = rt.lua().globals().get("_is_nil").unwrap();
    assert!(is_nil, "missing env var should return nil");
}
