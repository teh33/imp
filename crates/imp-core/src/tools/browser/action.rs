use serde_json::{json, Map, Value};

use crate::reference_monitor::ToolActionKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserAction {
    Start,
    Stop,
    Navigate,
    Observe,
    Markdown,
    Links,
    StructuredData,
    Forms,
    Extract,
    Click,
    Fill,
    Press,
    Select,
    Check,
    Scroll,
    Wait,
    GetUrl,
    Console,
}

impl BrowserAction {
    pub(crate) fn parse(params: &Value) -> Result<Self, String> {
        let action = params
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| "browser requires an action".to_string())?;
        match action {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "navigate" => Ok(Self::Navigate),
            "observe" => Ok(Self::Observe),
            "markdown" => Ok(Self::Markdown),
            "links" => Ok(Self::Links),
            "structured_data" => Ok(Self::StructuredData),
            "forms" => Ok(Self::Forms),
            "extract" => Ok(Self::Extract),
            "click" => Ok(Self::Click),
            "fill" => Ok(Self::Fill),
            "press" => Ok(Self::Press),
            "select" => Ok(Self::Select),
            "check" => Ok(Self::Check),
            "scroll" => Ok(Self::Scroll),
            "wait" => Ok(Self::Wait),
            "get_url" => Ok(Self::GetUrl),
            "console" => Ok(Self::Console),
            other => Err(format!("unknown browser action: {other}")),
        }
    }

    pub(crate) fn is_readonly(self) -> bool {
        matches!(
            self,
            Self::Observe
                | Self::Markdown
                | Self::Links
                | Self::StructuredData
                | Self::Forms
                | Self::Extract
                | Self::GetUrl
                | Self::Console
        )
    }

    pub(crate) fn policy_kind(self) -> ToolActionKind {
        match self {
            Self::Navigate => ToolActionKind::Network,
            Self::Click | Self::Fill | Self::Press | Self::Select | Self::Check | Self::Scroll => {
                ToolActionKind::Browser
            }
            _ => ToolActionKind::Read,
        }
    }

    pub(crate) fn validate(self, params: &Value) -> Result<(), String> {
        match self {
            Self::Navigate => require_string(params, "url"),
            Self::Click => require_target(params),
            Self::Fill | Self::Select => {
                require_target(params)?;
                require_string(params, "value")
            }
            Self::Press => require_string(params, "key"),
            Self::Check => {
                require_target(params)?;
                params
                    .get("checked")
                    .and_then(Value::as_bool)
                    .map(|_| ())
                    .ok_or_else(|| format!("browser {} requires checked", action_name(self)))
            }
            Self::Wait => require_string(params, "selector"),
            Self::Extract => require_string(params, "schema"),
            _ => Ok(()),
        }
    }

    pub(crate) fn mcp_call(self, params: &Value, default_timeout: u64) -> McpCall {
        let timeout = params
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(default_timeout)
            .clamp(100, 120_000) as u32;
        let mut args = Map::new();
        copy_string(params, &mut args, "selector", "selector");
        copy_u64(params, &mut args, "backend_node_id", "backendNodeId");

        match self {
            Self::Navigate => {
                copy_string(params, &mut args, "url", "url");
                args.insert("timeout".into(), json!(timeout));
                McpCall::new("goto", args)
            }
            Self::Observe => McpCall::new("interactiveElements", args),
            Self::Markdown => {
                copy_u64(params, &mut args, "max_bytes", "maxBytes");
                McpCall::new("markdown", args)
            }
            Self::Links => McpCall::new("links", args),
            Self::StructuredData => McpCall::new("structuredData", args),
            Self::Forms => McpCall::new("detectForms", args),
            Self::Extract => {
                copy_string(params, &mut args, "schema", "schema");
                McpCall::new("extract", args)
            }
            Self::Click => McpCall::new("click", args),
            Self::Fill => {
                copy_string(params, &mut args, "value", "value");
                McpCall::new("fill", args)
            }
            Self::Press => {
                copy_string(params, &mut args, "key", "key");
                McpCall::new("press", args)
            }
            Self::Select => {
                copy_string(params, &mut args, "value", "value");
                McpCall::new("selectOption", args)
            }
            Self::Check => {
                if let Some(checked) = params.get("checked").and_then(Value::as_bool) {
                    args.insert("checked".into(), json!(checked));
                }
                McpCall::new("setChecked", args)
            }
            Self::Scroll => {
                copy_i64(params, &mut args, "x", "x");
                copy_i64(params, &mut args, "y", "y");
                McpCall::new("scroll", args)
            }
            Self::Wait => {
                copy_string(params, &mut args, "selector", "selector");
                args.insert("timeout".into(), json!(timeout));
                McpCall::new("waitForSelector", args)
            }
            Self::GetUrl => McpCall::new("getUrl", args),
            Self::Console => McpCall::new("consoleLogs", args),
            Self::Start | Self::Stop => McpCall::new("getUrl", args),
        }
    }
}

#[derive(Debug)]
pub(crate) struct McpCall {
    pub(crate) tool: &'static str,
    pub(crate) arguments: Value,
}

impl McpCall {
    fn new(tool: &'static str, arguments: Map<String, Value>) -> Self {
        Self {
            tool,
            arguments: Value::Object(arguments),
        }
    }
}

fn require_string(params: &Value, field: &str) -> Result<(), String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|_| ())
        .ok_or_else(|| format!("browser action requires {field}"))
}

fn require_target(params: &Value) -> Result<(), String> {
    let selector = params
        .get("selector")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    let node = params
        .get("backend_node_id")
        .and_then(Value::as_u64)
        .is_some_and(|value| value > 0);
    if selector || node {
        Ok(())
    } else {
        Err("browser action requires selector or backend_node_id".into())
    }
}

fn action_name(action: BrowserAction) -> &'static str {
    match action {
        BrowserAction::Check => "check",
        _ => "action",
    }
}

fn copy_string(params: &Value, out: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = params.get(from).and_then(Value::as_str) {
        out.insert(to.into(), json!(value));
    }
}

fn copy_u64(params: &Value, out: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = params.get(from).and_then(Value::as_u64) {
        out.insert(to.into(), json!(value));
    }
}

fn copy_i64(params: &Value, out: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = params.get(from).and_then(Value::as_i64) {
        out.insert(to.into(), json!(value));
    }
}
