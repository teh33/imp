use std::sync::{Arc, Mutex};
use std::time::Instant;

use imp_core::config::{AgentMode, Config};
use imp_core::tools::bash::BashTool;
use imp_core::tools::scan::ScanTool;
use imp_core::tools::{AnchorStore, CheckpointState, FileCache, FileTracker, Tool, ToolContext};
use imp_core::ui::NullInterface;
use imp_core::workflow_review::TurnWorkflowReviewAccumulator;
use imp_llm::ContentBlock;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

fn approx_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

fn output_text(output: imp_core::tools::ToolOutput) -> String {
    output
        .content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_context(cwd: std::path::PathBuf) -> ToolContext {
    let (update_tx, _update_rx) = tokio::sync::mpsc::channel(16);
    let (command_tx, _command_rx) = tokio::sync::mpsc::channel(16);
    ToolContext {
        cwd,
        cancelled: Arc::new(AtomicBool::new(false)),
        update_tx,
        command_tx,
        ui: Arc::new(NullInterface),
        file_cache: Arc::new(FileCache::new()),
        checkpoint_state: Arc::new(CheckpointState::new()),
        file_tracker: Arc::new(Mutex::new(FileTracker::new())),
        anchor_store: Arc::new(AnchorStore::new()),
        lua_tool_loader: None,
        mode: AgentMode::Full,
        read_max_lines: 500,
        turn_workflow_review: Arc::new(Mutex::new(TurnWorkflowReviewAccumulator::default())),
        config: Arc::new(Config::default()),
        run_policy: Default::default(),
        supporting_provenance: Vec::new(),
    }
}

#[derive(Serialize)]
struct Measurement {
    task: &'static str,
    tool: &'static str,
    ms: u128,
    chars: usize,
    approx_tokens: usize,
    lines: usize,
    first_lines: Vec<String>,
}

async fn measure_tool<T: Tool>(
    task: &'static str,
    tool_name: &'static str,
    tool: &T,
    args: Value,
    ctx: ToolContext,
) -> Result<Measurement, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let output = output_text(tool.execute(tool_name, args, ctx).await?);
    Ok(Measurement {
        task,
        tool: tool_name,
        ms: started.elapsed().as_millis(),
        chars: output.len(),
        approx_tokens: approx_tokens(&output),
        lines: output.lines().count(),
        first_lines: output.lines().take(5).map(str::to_string).collect(),
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let ctx = tool_context(cwd);
    let scan = ScanTool;
    let bash = BashTool;

    let tasks: Vec<(&'static str, Value, Vec<(&'static str, &'static str)>)> = vec![
        (
            "exact private function definition",
            json!({"action":"search","query":"merge_scan_result_preserving_existing","max_results":5}),
            vec![
                ("rg", "rg -n 'merge_scan_result_preserving_existing' ."),
                ("git-grep", "git grep -n 'merge_scan_result_preserving_existing'"),
            ],
        ),
        (
            "broad type definition",
            json!({"action":"search","query":"AgentBuilder","max_results":5}),
            vec![
                ("rg", "rg -n 'AgentBuilder' ."),
                ("git-grep", "git grep -n 'AgentBuilder'"),
            ],
        ),
        (
            "concept query",
            json!({"action":"search","query":"workflow mutation","max_results":8}),
            vec![
                ("rg", "rg -n 'workflow|mutation' crates docs"),
                ("git-grep", "git grep -En 'workflow|mutation' -- crates docs"),
            ],
        ),
        (
            "likely tests for symbol",
            json!({"action":"tests","target":"crates/imp-core/src/tools/scan/mod.rs#collect_source_files","max_results":10}),
            vec![
                ("rg", "rg -n 'collect_source_files|scan_.*source|source_files' crates/imp-core/src"),
                ("git-grep", "git grep -En 'collect_source_files|scan_.*source|source_files' -- crates/imp-core/src"),
            ],
        ),
        (
            "related code for workflow action",
            json!({"action":"related","target":"crates/imp-core/src/tools/workflow.rs#WorkflowAction","max_results":10}),
            vec![
                ("rg", "rg -n 'WorkflowAction|workflow.*action|action.*workflow' crates/imp-core/src"),
                ("git-grep", "git grep -En 'WorkflowAction|workflow.*action|action.*workflow' -- crates/imp-core/src"),
            ],
        ),
        (
            "extract exact symbol body",
            json!({"action":"extract","target":"crates/imp-core/src/tools/scan/mod.rs#collect_source_files"}),
            vec![
                ("rg", "rg -n -C 20 'fn collect_source_files' crates/imp-core/src/tools/scan/mod.rs"),
                ("git-grep", "git grep -n -C 20 'fn collect_source_files' -- crates/imp-core/src/tools/scan/mod.rs"),
            ],
        ),
    ];

    let mut measurements: Vec<Measurement> = Vec::new();
    for (task, scan_args, shell_commands) in tasks {
        measurements.push(measure_tool(task, "scan", &scan, scan_args, ctx.clone()).await?);
        for (tool_name, command) in shell_commands {
            measurements.push(
                measure_tool(
                    task,
                    tool_name,
                    &bash,
                    json!({"command": command}),
                    ctx.clone(),
                )
                .await?,
            );
        }
    }

    println!("{}", serde_json::to_string_pretty(&measurements)?);
    Ok(())
}
