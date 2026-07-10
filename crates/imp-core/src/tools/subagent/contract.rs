use std::path::PathBuf;

use crate::agent::SubagentInput;
use crate::tools::ToolContext;

pub(super) fn child_prompt(input: &SubagentInput) -> String {
    let files = input
        .context
        .files
        .iter()
        .map(|file| {
            format!(
                "- {}{}",
                file.path.display(),
                file.note
                    .as_deref()
                    .map(|note| format!(": {note}"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Role: {:?}\nObjective: {}\nInstructions:\n{}\nContext messages:\n{}\nContext files:\n{}\nAllowed paths: {}\nWritable paths: {}\nOutput contract: {}\nMerge policy: {:?}",
        input.role,
        input.objective,
        input.context.instructions.join("\n"),
        input.context.messages.join("\n"),
        files,
        paths(&input.resource_limits.allowed_paths),
        paths(&input.resource_limits.writable_paths),
        input.output_contract.as_deref().unwrap_or("Report a structured final outcome."),
        input.merge_policy,
    )
}
pub(super) fn contract_args(input: &SubagentInput, ctx: &ToolContext) -> Vec<String> {
    let mut args = Vec::new();
    for tool in ctx.run_policy.allowed_tools() {
        args.extend(["--allow-tool".into(), tool.clone()]);
    }
    for tool in ctx.run_policy.denied_tools() {
        args.extend(["--deny-tool".into(), tool.clone()]);
    }
    if input.resource_limits.writable_paths.is_empty() {
        args.extend(["--deny-write".into(), "**".into()]);
    } else {
        for path in &input.resource_limits.writable_paths {
            args.extend(["--allow-write".into(), path.display().to_string()]);
        }
    }
    for pattern in ctx.run_policy.denied_write_patterns() {
        args.extend(["--deny-write".into(), pattern.clone()]);
    }
    args
}

pub(super) fn unsupported_limits(input: &SubagentInput) -> Vec<String> {
    [
        input
            .resource_limits
            .max_model_tokens
            .map(|value| format!("max_model_tokens={value}")),
        input
            .resource_limits
            .max_tool_calls
            .map(|value| format!("max_tool_calls={value}")),
        input
            .resource_limits
            .max_parallel_children
            .map(|value| format!("max_parallel_children={value}")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(super) fn child_environment(
    input: &SubagentInput,
    child_args: &[String],
) -> Vec<(String, String)> {
    let unsupported = [
        input
            .resource_limits
            .max_model_tokens
            .map(|value| format!("max_model_tokens={value}")),
        input
            .resource_limits
            .max_tool_calls
            .map(|value| format!("max_tool_calls={value}")),
        input
            .resource_limits
            .max_parallel_children
            .map(|value| format!("max_parallel_children={value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",");
    vec![
        (
            "IMP_SUBAGENT_ALLOWED_PATHS".into(),
            paths(&input.resource_limits.allowed_paths),
        ),
        (
            "IMP_SUBAGENT_TIMEOUT_SECONDS".into(),
            input
                .resource_limits
                .timeout_seconds
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        ("IMP_SUBAGENT_UNSUPPORTED_LIMITS".into(), unsupported),
        ("IMP_SUBAGENT_CHILD_ARGS".into(), child_args.join("\u{1f}")),
    ]
}

fn paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
