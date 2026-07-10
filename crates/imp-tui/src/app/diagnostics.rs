use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::DisplayToolCall;

#[derive(Debug, Clone)]
pub(super) struct TuiTrace {
    pub(super) path: PathBuf,
}

impl TuiTrace {
    pub(super) fn from_env() -> Option<Self> {
        Self::from_env_value(std::env::var_os("IMP_TUI_TRACE"))
    }

    pub(super) fn from_env_value(value: Option<std::ffi::OsString>) -> Option<Self> {
        value
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| Self { path })
    }

    pub(super) fn log(&self, message: impl AsRef<str>) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{} {}", imp_llm::now(), message.as_ref());
        }
    }
}

pub(super) fn trace_tui_to(trace: Option<&TuiTrace>, message: impl AsRef<str>) {
    if let Some(trace) = trace {
        trace.log(message);
    }
}

pub(super) fn selected_read_file_path_from_tool(
    tc: Option<&DisplayToolCall>,
    cwd: &Path,
) -> Option<PathBuf> {
    let tc = tc?;
    if tc.name != "read" {
        return None;
    }

    let path = tc.details.get("path")?.as_str()?.trim();
    if path.is_empty() {
        return None;
    }

    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

pub(super) fn open_path_in_editor(path: &Path) -> std::io::Result<()> {
    let editor = std::env::var_os("VISUAL").or_else(|| std::env::var_os("EDITOR"));
    if let Some(editor) = editor.filter(|value| !value.is_empty()) {
        return std::process::Command::new(editor)
            .arg(path)
            .spawn()
            .map(|_| ());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
}
