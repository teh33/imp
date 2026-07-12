use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::error::{Error, Result};

use super::ShellParamDef;

pub(super) fn validate_required_params(
    defs: &HashMap<String, ShellParamDef>,
    provided: &Map<String, Value>,
) -> Result<()> {
    let mut missing = defs
        .iter()
        .filter(|(name, def)| !def.optional && provided.get(*name).is_none_or(Value::is_null))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    missing.sort();
    if missing.is_empty() {
        return Ok(());
    }
    Err(Error::Tool(format!(
        "missing required parameter(s): {}",
        missing.join(", ")
    )))
}

pub(super) fn interpolate_arg(
    template: &str,
    defs: &HashMap<String, ShellParamDef>,
    provided: &Map<String, Value>,
) -> Result<String> {
    let mut result = String::new();
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        result.push_str(&remaining[..start]);
        let after_start = &remaining[start + 1..];
        let end = after_start.find('}').ok_or_else(|| {
            Error::Tool(format!(
                "unclosed placeholder in shell tool argument: {template}"
            ))
        })?;
        result.push_str(&resolve_placeholder(&after_start[..end], defs, provided)?);
        remaining = &after_start[end + 1..];
    }
    result.push_str(remaining);
    Ok(result)
}

fn resolve_placeholder(
    placeholder: &str,
    defs: &HashMap<String, ShellParamDef>,
    provided: &Map<String, Value>,
) -> Result<String> {
    let (name, default) = placeholder
        .split_once('|')
        .map_or((placeholder, None), |(name, default)| (name, Some(default)));
    if name.is_empty() {
        return Err(Error::Tool(
            "empty placeholder in shell tool argument".into(),
        ));
    }
    if let Some(value) = provided.get(name).filter(|value| !value.is_null()) {
        return stringify_param_value(name, value);
    }
    if let Some(default) = default {
        return Ok(default.to_string());
    }
    if defs.get(name).is_some_and(|def| def.optional) {
        return Ok(String::new());
    }
    Err(Error::Tool(format!(
        "missing required parameter for placeholder: {name}"
    )))
}

fn stringify_param_value(name: &str, value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        _ => Err(Error::Tool(format!(
            "parameter '{name}' must be a string, number, or boolean"
        ))),
    }
}
