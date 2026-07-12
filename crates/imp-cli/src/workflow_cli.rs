use clap::{Args, Subcommand, ValueEnum};
use imp_core::config::AgentMode;
use imp_core::config::Config;
use imp_core::tools::{
    AnchorStore, CheckpointState, FileCache, FileTracker, Tool, ToolContext, ToolOutput,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[derive(Subcommand, Debug)]
pub(crate) enum WorkflowCommand {
    /// List workflows under .imp/workflows
    List,
    /// Show a workflow summary
    Show(WorkflowTargetArgs),
    /// Validate one workflow, or all workflows when no id is provided
    Validate(WorkflowValidateArgs),
    /// Show the next runnable workflow step
    Run(WorkflowTargetArgs),
    /// Update a workflow status path and append an audit event
    Update(WorkflowUpdateArgs),
}

#[derive(Args, Debug)]
pub(crate) struct WorkflowTargetArgs {
    /// Workflow id under .imp/workflows
    pub(crate) id: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct WorkflowValidateArgs {
    /// Workflow id under .imp/workflows
    pub(crate) id: Option<String>,
    /// Validation mode
    #[arg(long, default_value = "strict")]
    pub(crate) mode: WorkflowValidationModeArg,
}

#[derive(Args, Debug)]
pub(crate) struct WorkflowUpdateArgs {
    /// Workflow id under .imp/workflows
    pub(crate) id: String,
    /// Workflow object path to replace, e.g. steps.verify.status
    pub(crate) path: String,
    /// Replacement status value
    pub(crate) value: String,
    /// Reason recorded in events.jsonl
    #[arg(long)]
    pub(crate) reason: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum WorkflowValidationModeArg {
    Draft,
    Strict,
}

impl WorkflowValidationModeArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Strict => "strict",
        }
    }
}

pub(crate) async fn run_command(command: &WorkflowCommand) -> imp_core::Result<()> {
    let (action, id, mode, path, value, reason) = match command {
        WorkflowCommand::List => ("list", None, None, None, None, None),
        WorkflowCommand::Show(args) => ("show", args.id.as_deref(), None, None, None, None),
        WorkflowCommand::Validate(args) => (
            "validate",
            args.id.as_deref(),
            Some(args.mode.as_str()),
            None,
            None,
            None,
        ),
        WorkflowCommand::Run(args) => ("run", args.id.as_deref(), None, None, None, None),
        WorkflowCommand::Update(args) => (
            "update",
            Some(args.id.as_str()),
            None,
            Some(args.path.as_str()),
            Some(args.value.as_str()),
            Some(args.reason.as_str()),
        ),
    };

    let mut params = serde_json::Map::from_iter([("action".to_string(), json!(action))]);
    if let Some(id) = id {
        params.insert("id".to_string(), json!(id));
    }
    if let Some(mode) = mode {
        params.insert("mode".to_string(), json!(mode));
    }
    if let Some(path) = path {
        params.insert("path".to_string(), json!(path));
    }
    if let Some(value) = value {
        params.insert("value".to_string(), json!(value));
    }
    if let Some(reason) = reason {
        params.insert("reason".to_string(), json!(reason));
    }

    let output = run_workflow_tool(Value::Object(params), AgentMode::Full).await?;
    print_tool_output(&output);
    if output.is_error {
        return Err(imp_core::error::Error::Tool(
            "workflow command failed".into(),
        ));
    }
    Ok(())
}

async fn run_workflow_tool(params: Value, mode: AgentMode) -> imp_core::Result<ToolOutput> {
    let (tx, _rx) = mpsc::channel(16);
    let (command_tx, _command_rx) = mpsc::channel(16);
    let cwd = std::env::current_dir()?;
    let tool = imp_core::tools::workflow::WorkflowTool;
    tool.execute(
        "cli-workflow",
        params,
        ToolContext {
            cwd,
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_tx: tx,
            command_tx,
            ui: std::sync::Arc::new(imp_core::ui::NullInterface),
            file_cache: std::sync::Arc::new(FileCache::new()),
            checkpoint_state: std::sync::Arc::new(CheckpointState::new()),
            file_tracker: std::sync::Arc::new(std::sync::Mutex::new(FileTracker::new())),
            anchor_store: std::sync::Arc::new(AnchorStore::new()),
            lua_tool_loader: None,
            mode,
            read_max_lines: 500,
            turn_workflow_review: std::sync::Arc::new(std::sync::Mutex::new(
                imp_core::workflow_review::TurnWorkflowReviewAccumulator::default(),
            )),
            config: std::sync::Arc::new(Config::default()),
            run_policy: Default::default(),
            supporting_provenance: Vec::new(),
        },
    )
    .await
}

fn print_tool_output(output: &ToolOutput) {
    for block in &output.content {
        if let imp_llm::ContentBlock::Text { text } = block {
            println!("{text}");
        }
    }
}
