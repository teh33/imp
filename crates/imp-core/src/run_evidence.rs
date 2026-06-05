use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::{AgentEvent, RunFinalStatus};
use crate::error::Result;

const VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct RunArtifacts {
    root: PathBuf,
}

impl RunArtifacts {
    pub fn create(root: impl Into<PathBuf>) -> Result<Self> {
        let artifacts = Self { root: root.into() };
        fs::create_dir_all(&artifacts.root)?;
        Ok(artifacts)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn events_path(&self) -> PathBuf {
        self.root.join("events.jsonl")
    }

    pub fn evidence_html_path(&self) -> PathBuf {
        self.root.join("evidence.html")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIndexRecord {
    pub version: u32,
    pub run_id: String,
    pub cwd: PathBuf,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub status: Option<String>,
    pub objective: String,
    pub events_path: PathBuf,
    pub evidence_html_path: PathBuf,
}

impl RunIndexRecord {
    pub fn started(
        run_id: impl Into<String>,
        cwd: impl Into<PathBuf>,
        started_at: u64,
        objective: impl Into<String>,
        artifacts: &RunArtifacts,
    ) -> Self {
        Self {
            version: VERSION,
            run_id: run_id.into(),
            cwd: cwd.into(),
            started_at,
            completed_at: None,
            status: None,
            objective: objective.into(),
            events_path: artifacts.events_path(),
            evidence_html_path: artifacts.evidence_html_path(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvent {
    pub version: u32,
    pub run_id: String,
    pub timestamp: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl RunEvent {
    pub fn from_agent_event(run_id: &str, event: &AgentEvent) -> Self {
        let mut run_event = Self {
            version: VERSION,
            run_id: run_id.to_string(),
            timestamp: imp_llm::now(),
            kind: agent_event_kind(event).to_string(),
            turn: None,
            tool_call_id: None,
            tool_name: None,
            status: None,
            summary: None,
        };

        match event {
            AgentEvent::AgentStart { model, timestamp } => {
                run_event.timestamp = *timestamp;
                run_event.summary = Some(format!("model={model}"));
            }
            AgentEvent::AgentEnd { status, .. } => {
                run_event.status = Some(status_label(status));
            }
            AgentEvent::TurnStart { index }
            | AgentEvent::TurnAssessment { index, .. }
            | AgentEvent::TurnEnd { index, .. } => {
                run_event.turn = Some(*index);
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                ..
            } => {
                run_event.tool_call_id = Some(tool_call_id.clone());
                run_event.tool_name = Some(tool_name.clone());
            }
            AgentEvent::ToolOutputDelta { tool_call_id, text } => {
                run_event.tool_call_id = Some(tool_call_id.clone());
                run_event.summary = Some(truncate(text, 240));
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                ..
            } => {
                run_event.tool_call_id = Some(tool_call_id.clone());
                run_event.tool_name = Some(result.tool_name.clone());
                run_event.status = Some(if result.is_error { "error" } else { "ok" }.into());
            }
            AgentEvent::ContextUsageUpdated {
                used,
                display_window,
                input_limit,
                system_tokens,
                tool_definition_tokens,
                message_tokens,
                output_tokens,
                observed_input_limit,
            } => {
                run_event.summary = Some(format!(
                    "{used}/{display_window} tokens (limit {input_limit}, system {system_tokens}, tools {tool_definition_tokens}, messages {message_tokens}, output {output_tokens}, observed ceiling {observed})",
                    observed = observed_input_limit
                        .map(|limit| limit.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ));
            }
            AgentEvent::Warning { message } => {
                run_event.summary = Some(truncate(message, 240));
            }
            AgentEvent::Error { error } => {
                run_event.status = Some("error".into());
                run_event.summary = Some(truncate(error, 240));
            }
            AgentEvent::Timing { timing } => {
                run_event.turn = Some(timing.turn);
                run_event.summary = Some(timing.stage.as_str().into());
            }
            AgentEvent::RecoveryCheckpoint { checkpoint } => {
                run_event.turn = Some(checkpoint.turn);
                run_event.tool_call_id = checkpoint.tool_call_id.clone();
                run_event.tool_name = checkpoint.tool_name.clone();
                run_event.status = checkpoint
                    .success
                    .map(|ok| if ok { "ok" } else { "error" }.into());
                run_event.summary = Some(checkpoint.kind.as_str().into());
            }
            AgentEvent::PolicyChecked { record } => {
                run_event.tool_name = Some(record.tool_name.clone());
                run_event.status = Some(
                    if record.decision.is_allowed() {
                        "allowed"
                    } else {
                        "denied"
                    }
                    .into(),
                );
                run_event.summary = Some(truncate(
                    &format!(
                        "action={:?} scope={:?}",
                        record.action_kind, record.resource_scope
                    ),
                    240,
                ));
            }
            AgentEvent::VerificationStarted { gate } => {
                run_event.status = Some("started".into());
                run_event.summary = Some(truncate(&gate.name, 240));
            }
            AgentEvent::VerificationCompleted {
                gate,
                closeout_effect,
            } => {
                run_event.status = Some(format!("{:?}", gate.status));
                run_event.summary = Some(truncate(
                    &format!("{} ({closeout_effect:?})", gate.name),
                    240,
                ));
            }
            AgentEvent::EvidenceWritten { path } => {
                run_event.summary = Some(path.display().to_string());
            }
            AgentEvent::MessageStart { .. }
            | AgentEvent::MessageDelta { .. }
            | AgentEvent::MessageEnd { .. }
            | AgentEvent::WorkflowControllerSnapshot { .. }
            | AgentEvent::WorktreeCreated { .. }
            | AgentEvent::WorktreeDiffCaptured { .. }
            | AgentEvent::WorktreeCloseout { .. } => {}
        }

        run_event
    }
}

pub struct RunEventWriter {
    file: File,
}

impl RunEventWriter {
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file })
    }

    pub fn write_event(&mut self, event: &RunEvent) -> Result<()> {
        let line = serde_json::to_string(event)?;
        writeln!(self.file, "{line}")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.file.flush()?;
        Ok(())
    }
}

pub fn append_index_record(path: impl AsRef<Path>, record: &RunIndexRecord) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(record)?;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn read_index_records(path: impl AsRef<Path>) -> Result<Vec<RunIndexRecord>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        records.push(serde_json::from_str(&line)?);
    }
    Ok(records)
}

pub fn write_evidence_html(
    path: impl AsRef<Path>,
    record: &RunIndexRecord,
    events: &[RunEvent],
) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }

    let mut html = String::new();
    html.push_str("<!doctype html><meta charset=\"utf-8\">");
    html.push_str("<title>imp evidence</title>");
    html.push_str("<style>body{font:14px system-ui,sans-serif;margin:2rem;max-width:960px}code,pre{background:#f6f6f6;padding:.2rem .35rem;border-radius:4px}table{border-collapse:collapse;width:100%}td,th{border-top:1px solid #ddd;padding:.4rem;text-align:left} .ok{color:#067d17}.error{color:#b00020}</style>");
    html.push_str("<h1>imp evidence</h1>");
    html.push_str("<dl>");
    html.push_str(&format!(
        "<dt>Run</dt><dd><code>{}</code></dd>",
        escape_html(&record.run_id)
    ));
    html.push_str(&format!(
        "<dt>CWD</dt><dd><code>{}</code></dd>",
        escape_html(&record.cwd.display().to_string())
    ));
    html.push_str(&format!(
        "<dt>Objective</dt><dd>{}</dd>",
        escape_html(&record.objective)
    ));
    if let Some(status) = &record.status {
        html.push_str(&format!("<dt>Status</dt><dd>{}</dd>", escape_html(status)));
    }
    html.push_str("</dl>");
    html.push_str("<h2>Timeline</h2><table><thead><tr><th>Time</th><th>Event</th><th>Tool</th><th>Status</th><th>Summary</th></tr></thead><tbody>");
    for event in events {
        let status_class = match event.status.as_deref() {
            Some("ok") | Some("done") => "ok",
            Some("error") | Some("blocked") => "error",
            _ => "",
        };
        html.push_str("<tr>");
        html.push_str(&format!("<td>{}</td>", event.timestamp));
        html.push_str(&format!(
            "<td><code>{}</code></td>",
            escape_html(&event.kind)
        ));
        html.push_str(&format!(
            "<td>{}</td>",
            escape_html(event.tool_name.as_deref().unwrap_or(""))
        ));
        html.push_str(&format!(
            "<td class=\"{}\">{}</td>",
            status_class,
            escape_html(event.status.as_deref().unwrap_or(""))
        ));
        html.push_str(&format!(
            "<td>{}</td>",
            escape_html(event.summary.as_deref().unwrap_or(""))
        ));
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html.push_str(&format!(
        "<p>Source: <code>{}</code></p>",
        escape_html(&record.events_path.display().to_string())
    ));
    fs::write(path, html)?;
    Ok(())
}

pub fn read_run_events(path: impl AsRef<Path>) -> Result<Vec<RunEvent>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        events.push(serde_json::from_str(&line)?);
    }
    Ok(events)
}

fn agent_event_kind(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::AgentStart { .. } => "run.started",
        AgentEvent::AgentEnd { .. } => "run.completed",
        AgentEvent::TurnStart { .. } => "turn.started",
        AgentEvent::TurnAssessment { .. } => "turn.assessed",
        AgentEvent::TurnEnd { .. } => "turn.completed",
        AgentEvent::MessageStart { .. } => "message.started",
        AgentEvent::MessageDelta { .. } => "message.delta",
        AgentEvent::MessageEnd { .. } => "message.completed",
        AgentEvent::ToolExecutionStart { .. } => "tool.started",
        AgentEvent::ToolOutputDelta { .. } => "tool.output_delta",
        AgentEvent::ToolExecutionEnd { .. } => "tool.completed",
        AgentEvent::ContextUsageUpdated { .. } => "context.usage",
        AgentEvent::Warning { .. } => "warning",
        AgentEvent::Timing { .. } => "timing",
        AgentEvent::RecoveryCheckpoint { .. } => "recovery.checkpoint",
        AgentEvent::PolicyChecked { .. } => "policy.checked",
        AgentEvent::VerificationStarted { .. } => "verification.started",
        AgentEvent::VerificationCompleted { .. } => "verification.completed",
        AgentEvent::EvidenceWritten { .. } => "evidence.written",
        AgentEvent::WorkflowControllerSnapshot { .. } => "workflow.controller_snapshot",
        AgentEvent::WorktreeCreated { .. } => "worktree.created",
        AgentEvent::WorktreeDiffCaptured { .. } => "worktree.diff_captured",
        AgentEvent::WorktreeCloseout { .. } => "worktree.closeout",
        AgentEvent::Error { .. } => "error",
    }
}

fn status_label(status: &RunFinalStatus) -> String {
    match status {
        RunFinalStatus::Done { .. } => "done".into(),
        RunFinalStatus::DoneWithConcerns { .. } => "done_with_concerns".into(),
        RunFinalStatus::Blocked { .. } => "blocked".into(),
        RunFinalStatus::NeedsUserInput { .. } => "needs_user_input".into(),
        RunFinalStatus::Cancelled => "cancelled".into(),
        RunFinalStatus::Failed { .. } => "failed".into(),
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_index_appends_and_reads_jsonl_records() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = RunArtifacts::create(temp.path().join("run_1")).unwrap();
        let index = temp.path().join("index.jsonl");
        let record =
            RunIndexRecord::started("run_1", temp.path(), 123, "fix evidence UX", &artifacts);

        append_index_record(&index, &record).unwrap();
        assert_eq!(read_index_records(&index).unwrap(), vec![record]);
    }

    #[test]
    fn run_events_are_jsonl_and_html_viewer_renders_timeline() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = RunArtifacts::create(temp.path().join("run_1")).unwrap();
        let mut writer = RunEventWriter::create(artifacts.events_path()).unwrap();
        let event = RunEvent {
            version: VERSION,
            run_id: "run_1".into(),
            timestamp: 123,
            kind: "tool.completed".into(),
            turn: Some(0),
            tool_call_id: Some("tc_1".into()),
            tool_name: Some("bash".into()),
            status: Some("ok".into()),
            summary: Some("cargo test".into()),
        };
        writer.write_event(&event).unwrap();
        writer.flush().unwrap();

        let events = read_run_events(artifacts.events_path()).unwrap();
        assert_eq!(events, vec![event]);

        let record = RunIndexRecord::started("run_1", temp.path(), 123, "test", &artifacts);
        write_evidence_html(artifacts.evidence_html_path(), &record, &events).unwrap();
        let html = fs::read_to_string(artifacts.evidence_html_path()).unwrap();
        assert!(html.contains("imp evidence"));
        assert!(html.contains("tool.completed"));
        assert!(html.contains("cargo test"));
    }
}
