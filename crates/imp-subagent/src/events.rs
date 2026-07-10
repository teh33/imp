use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::ChildStdin;

use serde_json::Value;

use crate::model::{Error, Record, Result, Status};
use crate::store::{load_record, now_ms, save_record};

pub(crate) fn send_child(stdin: &mut ChildStdin, kind: &str, prompt: String) -> Result<String> {
    let value = serde_json::json!({"type": kind, "content": prompt});
    serde_json::to_writer(&mut *stdin, &value)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(format!("turn_{}", now_ms()))
}
pub(crate) fn handle_event(
    record: &mut Record,
    active: &mut Option<String>,
    result: &mut String,
    event: Value,
) -> Result<()> {
    append_event(&record.artifacts.transcript, &event)?;
    if event["type"] == "message_end" && event["message"]["role"] == "assistant" {
        *result = event["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect();
    }
    if event["type"] != "agent_end" {
        return Ok(());
    }
    let status = event["status"]
        .get("type")
        .or_else(|| event.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("done");
    record.status = match status {
        "blocked" => Status::Blocked,
        "needs_user_input" | "needs_input" => Status::NeedsInput,
        "failed" => Status::Failed,
        "cancelled" => Status::Cancelled,
        _ => Status::Success,
    };
    record.summary = (!result.is_empty()).then(|| result.clone());
    if matches!(
        record.status,
        Status::Blocked | Status::NeedsInput | Status::Failed
    ) {
        record.diagnostics.push(
            event["status"]["message"]
                .as_str()
                .unwrap_or(status)
                .to_string(),
        );
    }
    record.updated_at_ms = now_ms();
    save_record(record)?;
    *active = None;
    Ok(())
}
pub(crate) fn remove_socket(state: &Path) -> Result<()> {
    let record = load_record(state)?;
    match fs::remove_file(record.socket) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn mark_worker_failed(state: &Path, error: &Error) -> Result<()> {
    let mut record = load_record(state)?;
    record.status = Status::Failed;
    record.diagnostics.push(error.to_string());
    record.updated_at_ms = now_ms();
    save_record(&record)
}

pub(crate) fn fail(record: &mut Record, message: String) -> Result<()> {
    record.status = Status::Failed;
    record.diagnostics.push(message);
    record.updated_at_ms = now_ms();
    save_record(record)
}
fn append_event(path: &Path, event: &Value) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, event)?;
    file.write_all(b"\n")?;
    Ok(())
}
