use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::child_process::{isolate_tokio_command, kill_tokio_process_group};

use super::{truncate_tail, Tool, ToolContext, ToolOutput, ToolUpdate, TruncationResult};
use crate::error::{Error, Result};
use crate::reference_monitor::{ToolActionKind, ToolMetadata};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum HypothesisResult {
    Supported,
    Refuted,
    Inconclusive,
    #[default]
    NotAssessed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum PrototypeOutcome {
    Promote,
    Discard,
    Iterate,
    #[default]
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum PrototypeRecordPolicy {
    #[default]
    None,
    Memory,
    Prototype,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PrototypeEvidence {
    claim: String,
    proof: String,
    artifact: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PrototypeObservation {
    prototype_id: String,
    question: String,
    parent_work: Option<String>,
    hypothesis: Option<String>,
    hypothesis_result: HypothesisResult,
    outcome: PrototypeOutcome,
    summary: String,
    evidence_required: Vec<String>,
    evidence: Vec<PrototypeEvidence>,
    learnings: Vec<String>,
    followups: Vec<String>,
    sandbox: PathBuf,
    artifacts: Vec<PathBuf>,
}

struct WorkStore;

impl WorkStore {
    fn open(_root: &Path) -> Self {
        Self
    }

    fn record_prototype_observation(
        &self,
        _policy: PrototypeRecordPolicy,
        _observation: &PrototypeObservation,
    ) -> std::result::Result<Option<PathBuf>, String> {
        Ok(None)
    }
}

const DEFAULT_TIMEOUT_SECS: u64 = 300;
const MAX_TIMEOUT_SECS: u64 = 1_800;
const MAX_OUTPUT_LINES: usize = 400;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PrototypeLanguage {
    Shell,
    Python,
    Rust,
    JavaScript,
    TypeScript,
    Go,
    Elixir,
    Ruby,
    Perl,
    Lua,
    Zig,
    Odin,
    Swift,
}

impl PrototypeLanguage {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "shell" | "sh" | "bash" => Ok(Self::Shell),
            "python" | "python3" | "py" => Ok(Self::Python),
            "rust" | "rs" => Ok(Self::Rust),
            "javascript" | "js" | "node" => Ok(Self::JavaScript),
            "typescript" | "ts" | "bun" | "deno" => Ok(Self::TypeScript),
            "go" | "golang" => Ok(Self::Go),
            "elixir" | "exs" => Ok(Self::Elixir),
            "ruby" | "rb" => Ok(Self::Ruby),
            "perl" | "pl" => Ok(Self::Perl),
            "lua" | "luajit" => Ok(Self::Lua),
            "zig" => Ok(Self::Zig),
            "odin" => Ok(Self::Odin),
            "swift" => Ok(Self::Swift),
            other => Err(Error::Tool(format!(
                "unsupported prototype language `{other}`; supported: shell, python, rust, javascript, typescript, go, elixir, ruby, perl, lua, zig, odin, swift"
            ))),
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Shell => "prototype.sh",
            Self::Python => "prototype.py",
            Self::Rust => "prototype.rs",
            Self::JavaScript => "prototype.js",
            Self::TypeScript => "prototype.ts",
            Self::Go => "prototype.go",
            Self::Elixir => "prototype.exs",
            Self::Ruby => "prototype.rb",
            Self::Perl => "prototype.pl",
            Self::Lua => "prototype.lua",
            Self::Zig => "prototype.zig",
            Self::Odin => "prototype.odin",
            Self::Swift => "prototype.swift",
        }
    }

    fn command(self, file_name: &str) -> String {
        match self {
            Self::Shell => format!("bash {file_name}"),
            Self::Python => format!("python3 {file_name}"),
            Self::Rust => format!("rustc {file_name} -o prototype && ./prototype"),
            Self::JavaScript => format!("node {file_name}"),
            Self::TypeScript => format!(
                "if command -v node >/dev/null 2>&1 && node --experimental-strip-types {file_name}; then :; elif command -v bun >/dev/null 2>&1; then bun run {file_name}; elif command -v deno >/dev/null 2>&1; then deno run --quiet {file_name}; else echo 'typescript prototypes require node with --experimental-strip-types, bun, or deno' >&2; exit 127; fi"
            ),
            Self::Go => format!("go run {file_name}"),
            Self::Elixir => format!("elixir {file_name}"),
            Self::Ruby => format!("ruby {file_name}"),
            Self::Perl => format!("perl {file_name}"),
            Self::Lua => format!(
                "if command -v lua >/dev/null 2>&1; then lua {file_name}; elif command -v luajit >/dev/null 2>&1; then luajit {file_name}; else echo 'lua prototypes require lua or luajit' >&2; exit 127; fi"
            ),
            Self::Zig => format!("zig run {file_name}"),
            Self::Odin => "odin run . -file".to_string(),
            Self::Swift => format!("swift {file_name}"),
        }
    }
}

pub struct PrototypeTool;

#[async_trait]
impl Tool for PrototypeTool {
    fn name(&self) -> &str {
        "prototype"
    }

    fn label(&self) -> &str {
        "Prototype"
    }

    fn description(&self) -> &str {
        "Run a bounded disposable code experiment in an isolated scratch directory and return structured evidence/learnings. Use for quick prototype.run-style experiments whose code is expected to be deleted or explicitly promoted later."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run"],
                    "description": "Prototype action. Currently only run is supported; future actions may include create, observe, promote, discard, and cleanup."
                },
                "question": {
                    "type": "string",
                    "description": "Engineering question this prototype should answer."
                },
                "parent_work": {
                    "type": "string",
                    "description": "Optional parent task/epic/work item this prototype is answering uncertainty for."
                },
                "hypothesis": {
                    "type": "string",
                    "description": "Optional expected answer to test."
                },
                "language": {
                    "type": "string",
                    "enum": ["shell", "python", "rust", "javascript", "typescript", "go", "elixir", "ruby", "perl", "lua", "zig", "odin", "swift"],
                    "description": "Scratch language/runtime for the experiment. JavaScript uses node; TypeScript prefers node --experimental-strip-types with bun/deno fallback; Go uses go run; Elixir uses elixir; other languages use their standard CLI when installed."
                },
                "code": {
                    "type": "string",
                    "description": "Complete disposable prototype code to write and run."
                },
                "evidence_required": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Evidence the experiment should try to produce."
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds, capped at 1800."
                },
                "evidence": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "claim": { "type": "string" },
                            "proof": { "type": "string" },
                            "artifact": { "type": "string" }
                        },
                        "required": ["claim", "proof"]
                    },
                    "description": "Structured evidence produced or expected from the prototype, beyond raw output."
                },
                "learnings": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Durable learnings to record if this result matters."
                },
                "followups": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Follow-up production tasks or questions suggested by the prototype."
                },
                "hypothesis_result": {
                    "type": "string",
                    "enum": ["supported", "refuted", "inconclusive", "not_assessed"],
                    "description": "Caller assessment of the hypothesis after the experiment. Defaults from exit status if omitted."
                },
                "recommended_action": {
                    "type": "string",
                    "enum": ["promote", "discard", "iterate", "inconclusive"],
                    "description": "Caller recommendation for the prototype result. Defaults from exit status if omitted."
                },
                "record": {
                    "type": "string",
                    "enum": ["none", "memory", "prototype"],
                    "description": "Durable recording policy. none keeps this ephemeral; memory appends learnings to .imp/work/memory.md; prototype appends full observation to .imp/work/prototypes.md."
                },
                "keep_artifacts": {
                    "type": "boolean",
                    "description": "Keep sandbox artifacts for review. Defaults to true."
                }
            },
            "required": ["question", "language", "code"]
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn policy_metadata(&self) -> ToolMetadata {
        let mut metadata = ToolMetadata::new(self.name(), ToolActionKind::Execute);
        metadata.supports_sandbox = true;
        metadata.external_side_effect = false;
        metadata.workspace_write = false;
        metadata.default_requires_approval = false;
        metadata
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let action = params
            .get("action")
            .and_then(|value| value.as_str())
            .unwrap_or("run");
        if action != "run" {
            return Err(Error::Tool(format!(
                "unsupported prototype action `{action}`; currently only `run` is supported"
            )));
        }
        let question = required_str(&params, "question")?.trim().to_string();
        let parent_work = optional_string(&params, "parent_work");
        let hypothesis = params
            .get("hypothesis")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let language = PrototypeLanguage::parse(required_str(&params, "language")?.trim())?;
        let code = required_str(&params, "code")?.to_string();
        let timeout_secs = params
            .get("timeout")
            .and_then(|value| value.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);
        let keep_artifacts = params
            .get("keep_artifacts")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        let evidence_required = string_array(&params, "evidence_required");
        let learnings = string_array(&params, "learnings");
        let followups = string_array(&params, "followups");
        let requested_hypothesis_result = params
            .get("hypothesis_result")
            .and_then(|value| value.as_str())
            .map(parse_hypothesis_result)
            .transpose()?;
        let requested_action = params
            .get("recommended_action")
            .and_then(|value| value.as_str())
            .map(parse_prototype_outcome)
            .transpose()?;
        let record_policy = params
            .get("record")
            .and_then(|value| value.as_str())
            .map(parse_record_policy)
            .transpose()?
            .unwrap_or_default();

        let sandbox = create_sandbox(&ctx.cwd)?;
        let evidence = parse_evidence(&params, &sandbox)?;
        let file_name = language.file_name();
        let code_path = sandbox.join(file_name);
        std::fs::write(&code_path, code)?;
        std::fs::write(
            sandbox.join("prototype.json"),
            serde_json::to_vec_pretty(&json!({
                "question": question,
                "parent_work": parent_work,
                "hypothesis": hypothesis,
                "language": language,
                "timeout_seconds": timeout_secs,
                "evidence_required": evidence_required,
            }))?,
        )?;

        let command = language.command(file_name);
        let run = run_sandbox_command(&command, timeout_secs, &sandbox, &ctx).await?;
        let output_path = sandbox.join("output.log");
        std::fs::write(&output_path, &run.output)?;

        let TruncationResult {
            content: output_preview,
            truncated,
            output_lines,
            total_lines,
            temp_file: _,
            ..
        } = truncate_tail(&run.output, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES);

        let hypothesis_result = requested_hypothesis_result.unwrap_or_else(|| {
            if run.timed_out {
                HypothesisResult::Inconclusive
            } else if hypothesis.is_some() && run.exit_code == 0 {
                HypothesisResult::Supported
            } else if hypothesis.is_some() {
                HypothesisResult::Refuted
            } else {
                HypothesisResult::NotAssessed
            }
        });
        let recommended_action =
            requested_action.unwrap_or(if run.timed_out || run.exit_code != 0 {
                PrototypeOutcome::Iterate
            } else {
                PrototypeOutcome::Inconclusive
            });
        let observation = PrototypeObservation {
            prototype_id: sandbox
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("P-unknown")
                .to_string(),
            question: question.clone(),
            parent_work: parent_work.clone(),
            hypothesis: hypothesis.clone(),
            hypothesis_result,
            outcome: recommended_action,
            summary: format!(
                "Prototype exited {} (timed_out: {}) while answering: {}",
                run.exit_code, run.timed_out, question
            ),
            evidence_required: evidence_required.clone(),
            evidence: evidence.clone(),
            learnings: learnings.clone(),
            followups: followups.clone(),
            sandbox: sandbox.clone(),
            artifacts: vec![
                code_path.clone(),
                output_path.clone(),
                sandbox.join("prototype.json"),
            ],
        };
        let recorded_path = WorkStore::open(&ctx.cwd)
            .record_prototype_observation(record_policy, &observation)
            .map_err(|error| Error::Tool(format!("failed to record prototype result: {error}")))?;

        let outcome = if run.timed_out {
            "inconclusive"
        } else if run.exit_code == 0 {
            "observed"
        } else {
            "failed"
        };

        let summary = format!(
            "Prototype answered `{}` with outcome `{}` (exit {}, timed_out: {}). Artifacts: {}",
            question,
            outcome,
            run.exit_code,
            run.timed_out,
            sandbox.display()
        );
        let mut text = summary.clone();
        if !evidence_required.is_empty() {
            text.push_str("\n\nEvidence requested:\n");
            for item in &evidence_required {
                text.push_str(&format!("- {item}\n"));
            }
        }
        if !learnings.is_empty() {
            text.push_str("\nLearnings:\n");
            for learning in &learnings {
                text.push_str(&format!("- {learning}\n"));
            }
        }
        if !followups.is_empty() {
            text.push_str("\nFollow-ups:\n");
            for followup in &followups {
                text.push_str(&format!("- {followup}\n"));
            }
        }
        if let Some(recorded_path) = &recorded_path {
            text.push_str(&format!(
                "\nRecorded prototype result to {}",
                recorded_path.display()
            ));
        }
        text.push_str("\nOutput:\n");
        text.push_str(&output_preview);
        if truncated {
            text.push_str(&format!(
                "\n[Output truncated: showing last {output_lines} of {total_lines} lines. Full output saved to {}]",
                output_path.display()
            ));
        }
        if !keep_artifacts {
            // Keep a summary in the returned details, then delete the disposable sandbox.
            let _ = std::fs::remove_dir_all(&sandbox);
            text.push_str("\n[Sandbox deleted after summary]");
        }

        Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text { text }],
            details: json!({
                "question": question,
                "parent_work": parent_work,
                "hypothesis": hypothesis,
                "hypothesis_result": hypothesis_result,
                "language": language,
                "sandbox": sandbox,
                "artifacts_kept": keep_artifacts,
                "artifacts": {
                    "code": code_path,
                    "output": output_path,
                    "metadata": sandbox.join("prototype.json"),
                },
                "command": command,
                "exit_code": run.exit_code,
                "timed_out": run.timed_out,
                "outcome": outcome,
                "recommended_action": recommended_action,
                "record": record_policy,
                "recorded_path": recorded_path,
                "evidence_required": evidence_required,
                "evidence": evidence,
                "learnings": learnings,
                "followups": followups,
                "truncated": truncated,
            }),
            is_error: run.timed_out || run.exit_code != 0,
        })
    }
}

fn required_str<'a>(params: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::Tool(format!("missing `{key}` parameter")))
}

fn optional_string(params: &serde_json::Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn string_array(params: &serde_json::Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::trim))
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_evidence(params: &serde_json::Value, sandbox: &Path) -> Result<Vec<PrototypeEvidence>> {
    let Some(items) = params.get("evidence").and_then(|value| value.as_array()) else {
        return Ok(Vec::new());
    };
    let mut evidence = Vec::with_capacity(items.len());
    for item in items {
        let claim = item
            .get("claim")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Tool("prototype evidence item missing `claim`".into()))?;
        let proof = item
            .get("proof")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Tool("prototype evidence item missing `proof`".into()))?;
        let artifact = item
            .get("artifact")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| sandbox.join(value));
        evidence.push(PrototypeEvidence {
            claim: claim.to_string(),
            proof: proof.to_string(),
            artifact,
        });
    }
    Ok(evidence)
}

fn parse_hypothesis_result(value: &str) -> Result<HypothesisResult> {
    match value {
        "supported" => Ok(HypothesisResult::Supported),
        "refuted" => Ok(HypothesisResult::Refuted),
        "inconclusive" => Ok(HypothesisResult::Inconclusive),
        "not_assessed" => Ok(HypothesisResult::NotAssessed),
        other => Err(Error::Tool(format!(
            "unsupported hypothesis_result `{other}`; expected supported, refuted, inconclusive, or not_assessed"
        ))),
    }
}

fn parse_prototype_outcome(value: &str) -> Result<PrototypeOutcome> {
    match value {
        "promote" => Ok(PrototypeOutcome::Promote),
        "discard" => Ok(PrototypeOutcome::Discard),
        "iterate" => Ok(PrototypeOutcome::Iterate),
        "inconclusive" => Ok(PrototypeOutcome::Inconclusive),
        other => Err(Error::Tool(format!(
            "unsupported recommended_action `{other}`; expected promote, discard, iterate, or inconclusive"
        ))),
    }
}

fn parse_record_policy(value: &str) -> Result<PrototypeRecordPolicy> {
    match value {
        "none" => Ok(PrototypeRecordPolicy::None),
        "memory" => Ok(PrototypeRecordPolicy::Memory),
        "prototype" => Ok(PrototypeRecordPolicy::Prototype),
        other => Err(Error::Tool(format!(
            "unsupported record policy `{other}`; expected none, memory, or prototype"
        ))),
    }
}

fn create_sandbox(cwd: &Path) -> std::io::Result<PathBuf> {
    let root = cwd.join(".tmp").join("imp-prototypes");
    std::fs::create_dir_all(&root)?;
    let sandbox = root.join(format!("P-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&sandbox)?;
    Ok(sandbox)
}

#[derive(Debug)]
struct SandboxRun {
    output: String,
    exit_code: i32,
    timed_out: bool,
}

async fn run_sandbox_command(
    command: &str,
    timeout_secs: u64,
    sandbox: &Path,
    ctx: &ToolContext,
) -> Result<SandboxRun> {
    if ctx.is_cancelled() {
        return Ok(SandboxRun {
            output: "[Prototype cancelled]".into(),
            exit_code: -1,
            timed_out: false,
        });
    }

    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(command)
        .current_dir(sandbox)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    isolate_tokio_command(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| Error::Tool(format!("failed to spawn prototype command: {error}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Tool("failed to capture prototype stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Tool("failed to capture prototype stderr".into()))?;
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut output = String::new();
    let mut timed_out = false;

    while !stdout_done || !stderr_done {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                kill_tokio_process_group(&child).await;
                let _ = child.kill().await;
                break;
            }
            line = stdout_reader.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => append_line(&mut output, &line, &ctx.update_tx).await,
                    _ => stdout_done = true,
                }
            }
            line = stderr_reader.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => append_line(&mut output, &line, &ctx.update_tx).await,
                    _ => stderr_done = true,
                }
            }
        }
    }

    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .ok()
        .and_then(|result| result.ok());
    let exit_code = status.and_then(|status| status.code()).unwrap_or(-1);
    Ok(SandboxRun {
        output,
        exit_code,
        timed_out,
    })
}

async fn append_line(
    output: &mut String,
    line: &str,
    update_tx: &tokio::sync::mpsc::Sender<ToolUpdate>,
) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(line);
    let _ = update_tx
        .send(ToolUpdate {
            content: vec![imp_llm::ContentBlock::Text {
                text: line.to_string(),
            }],
            details: serde_json::Value::Null,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::NullInterface;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn test_ctx(dir: &std::path::Path) -> (ToolContext, tokio::sync::mpsc::Receiver<ToolUpdate>) {
        let (tx, rx) = tokio::sync::mpsc::channel(1024);
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
        let ctx = ToolContext {
            cwd: dir.to_path_buf(),
            cancelled: Arc::new(AtomicBool::new(false)),
            update_tx: tx,
            command_tx: cmd_tx,
            ui: Arc::new(NullInterface),
            file_cache: Arc::new(crate::tools::FileCache::new()),
            checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
            file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
            anchor_store: Arc::new(crate::tools::AnchorStore::new()),
            lua_tool_loader: None,
            mode: crate::config::AgentMode::Full,
            read_max_lines: 500,
            turn_workflow_review: Arc::new(std::sync::Mutex::new(
                crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
            )),
            config: Arc::new(crate::config::Config::default()),
            run_policy: Default::default(),
            supporting_provenance: Vec::new(),
        };
        (ctx, rx)
    }

    #[tokio::test]
    async fn prototype_run_executes_python_in_sandbox() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _rx) = test_ctx(tmp.path());
        let tool = PrototypeTool;

        let result = tool
            .execute(
                "call-1",
                json!({
                    "question": "Can arithmetic run in a prototype sandbox?",
                    "language": "python",
                    "code": "print(2 + 2)",
                    "evidence_required": ["output is 4"],
                    "learnings": ["Prototype recording can persist structured learnings."],
                    "followups": ["Promote durable recording into imp-work store."],
                    "record": "prototype",
                    "timeout": 5
                }),
                ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.details["exit_code"], 0);
        assert_eq!(result.details["outcome"], "observed");
        assert_eq!(result.details["record"], "prototype");
        let recorded_path = result.details["recorded_path"].as_str().unwrap();
        assert!(std::path::Path::new(recorded_path).exists());
        assert!(std::fs::read_to_string(recorded_path)
            .unwrap()
            .contains("Prototype recording can persist structured learnings."));
        assert!(result.text_content().unwrap().contains('4'));
        let sandbox = result.details["sandbox"].as_str().unwrap();
        assert!(std::path::Path::new(sandbox).join("prototype.py").exists());
    }

    #[tokio::test]
    async fn prototype_run_executes_javascript_in_sandbox() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping javascript prototype test; node unavailable");
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _rx) = test_ctx(tmp.path());
        let tool = PrototypeTool;

        let result = tool
            .execute(
                "call-1",
                json!({
                    "question": "Can JavaScript run in a prototype sandbox?",
                    "language": "javascript",
                    "code": "console.log(['proto', 42].join(':'))",
                    "timeout": 5
                }),
                ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.details["exit_code"], 0);
        assert!(result.text_content().unwrap().contains("proto:42"));
        let sandbox = result.details["sandbox"].as_str().unwrap();
        assert!(std::path::Path::new(sandbox).join("prototype.js").exists());
    }

    #[tokio::test]
    async fn prototype_run_executes_typescript_with_available_runtime() {
        let has_runtime = std::process::Command::new("bun")
            .arg("--version")
            .output()
            .is_ok()
            || std::process::Command::new("deno")
                .arg("--version")
                .output()
                .is_ok();
        if !has_runtime {
            eprintln!("skipping typescript prototype test; bun/deno unavailable");
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _rx) = test_ctx(tmp.path());
        let tool = PrototypeTool;

        let result = tool
            .execute(
                "call-1",
                json!({
                    "question": "Can TypeScript run in a prototype sandbox?",
                    "language": "typescript",
                    "code": "const value: number = 21 * 2; console.log(`ts:${value}`);",
                    "timeout": 5
                }),
                ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.details["exit_code"], 0);
        assert!(result.text_content().unwrap().contains("ts:42"));
        let sandbox = result.details["sandbox"].as_str().unwrap();
        assert!(std::path::Path::new(sandbox).join("prototype.ts").exists());
    }

    #[tokio::test]
    async fn prototype_run_executes_go_in_sandbox() {
        if std::process::Command::new("go")
            .arg("version")
            .output()
            .is_err()
        {
            eprintln!("skipping go prototype test; go unavailable");
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _rx) = test_ctx(tmp.path());
        let tool = PrototypeTool;

        let result = tool
            .execute(
                "call-1",
                json!({
                    "question": "Can Go run in a prototype sandbox?",
                    "language": "go",
                    "code": "package main\nimport \"fmt\"\nfunc main() { fmt.Println(\"go:42\") }",
                    "timeout": 10
                }),
                ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.details["exit_code"], 0);
        assert!(result.text_content().unwrap().contains("go:42"));
        let sandbox = result.details["sandbox"].as_str().unwrap();
        assert!(std::path::Path::new(sandbox).join("prototype.go").exists());
    }

    #[tokio::test]
    async fn prototype_run_executes_elixir_in_sandbox() {
        if std::process::Command::new("elixir")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping elixir prototype test; elixir unavailable");
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _rx) = test_ctx(tmp.path());
        let tool = PrototypeTool;

        let result = tool
            .execute(
                "call-1",
                json!({
                    "question": "Can Elixir run in a prototype sandbox?",
                    "language": "elixir",
                    "code": "IO.puts(\"elixir:#{21 * 2}\")",
                    "timeout": 10
                }),
                ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.details["exit_code"], 0);
        assert!(result.text_content().unwrap().contains("elixir:42"));
        let sandbox = result.details["sandbox"].as_str().unwrap();
        assert!(std::path::Path::new(sandbox).join("prototype.exs").exists());
    }

    #[tokio::test]
    async fn prototype_run_can_delete_sandbox_after_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, _rx) = test_ctx(tmp.path());
        let tool = PrototypeTool;

        let result = tool
            .execute(
                "call-1",
                json!({
                    "question": "Can disposable artifacts be deleted?",
                    "language": "shell",
                    "code": "echo disposable",
                    "keep_artifacts": false,
                    "timeout": 5
                }),
                ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.details["artifacts_kept"], false);
        let sandbox = result.details["sandbox"].as_str().unwrap();
        assert!(!std::path::Path::new(sandbox).exists());
    }
}
