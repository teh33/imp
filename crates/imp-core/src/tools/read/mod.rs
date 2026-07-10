use std::path::Path;

use async_trait::async_trait;
use serde_json::json;

use super::{suggest_similar_files, truncate_head, Tool, ToolContext, ToolOutput};
use crate::error::Result;
use crate::tools::code_intel;

const MAX_BYTES: usize = 50_000;
const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg"];

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn label(&self) -> &str {
        "Read File"
    }
    fn description(&self) -> &str {
        "Read a file by path, line range, or semantic target."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "start_line": { "type": "integer", "minimum": 1 },
                "end_line": { "type": "integer", "minimum": 1 },
                "anchors": { "type": "boolean" },
                "target": { "type": "string" }
            },
            "required": ["path"]
        })
    }
    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let target = params["target"].as_str().unwrap_or("").trim();
        let mut raw_path_string = params["path"].as_str().unwrap_or("").trim().to_string();
        let mut semantic_symbol = params["symbol"].as_str().map(str::to_string);
        let mut semantic_line = None;
        if !target.is_empty() {
            if let Some((target_path, target_symbol)) = target.split_once('#') {
                raw_path_string = target_path.to_string();
                if !target_symbol.trim().is_empty() {
                    semantic_symbol = Some(target_symbol.trim().to_string());
                }
            } else if let Some((target_path, line)) = parse_line_target(target) {
                raw_path_string = target_path.to_string();
                semantic_line = Some(line);
            } else {
                raw_path_string = target.to_string();
            }
        }
        let raw_path = raw_path_string.trim_start_matches('@');

        if raw_path.is_empty() {
            return Ok(ToolOutput::error("Missing required parameter: path"));
        }

        let path = super::resolve_path(&ctx.cwd, raw_path);
        let mut range = parse_line_range(&params)?;

        if !path.exists() {
            let suggestions = suggest_similar_files(&ctx.cwd, raw_path);
            let mut msg = format!("File not found: {}", path.display());
            if !suggestions.is_empty() {
                msg.push_str("\n\nDid you mean:");
                for s in &suggestions {
                    msg.push_str(&format!("\n  {s}"));
                }
            }
            return Ok(ToolOutput::error(msg));
        }

        if path.is_dir() {
            return Ok(ToolOutput::error(format!(
                "Path is a directory, not a file: {}",
                path.display()
            )));
        }

        // Check for image files
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                return read_image(&path).await;
            }
        }

        let metadata = tokio::fs::metadata(&path).await?;
        if metadata.len() > MAX_TEXT_BYTES && range.is_none() {
            return Ok(ToolOutput::error(format!(
                "File is too large to read without a line range: {} ({} bytes). Use start_line/end_line to read a smaller range.",
                path.display(),
                metadata.len()
            )));
        }

        // Read raw bytes and check for binary
        let bytes = tokio::fs::read(&path).await?;
        let check_len = bytes.len().min(8192);
        if bytes[..check_len].contains(&0) {
            return Ok(ToolOutput::error(format!(
                "Binary file detected: {}. Cannot display binary content.",
                path.display()
            )));
        }

        let content = String::from_utf8_lossy(&bytes).into_owned();

        let expand_enclosing = params["expand"].as_str() == Some("enclosing_symbol");
        let mut semantic_details = serde_json::Value::Null;
        if let Some(symbol) = semantic_symbol.as_deref() {
            if let Some(mut block) = code_intel::extract_symbol(&content, &path, symbol) {
                block.file = path.clone();
                range = Some(LineRange {
                    start: block.start_line,
                    end: Some(block.end_line),
                });
                semantic_details = code_intel::block_details(&block);
            }
        } else if let Some(line) =
            semantic_line.or_else(|| expand_enclosing.then(|| range.map(|r| r.start)).flatten())
        {
            if let Some(mut blocks) =
                code_intel::extract_blocks_at_lines(&content, &path, &[line.saturating_sub(1)])
            {
                if let Some(mut block) = blocks.pop() {
                    block.file = path.clone();
                    range = Some(LineRange {
                        start: block.start_line,
                        end: Some(block.end_line),
                    });
                    semantic_details = code_intel::block_details(&block);
                }
            }
        }

        // Apply line range.
        let include_anchors = params["anchors"].as_bool().unwrap_or(false);

        let sliced = apply_line_range(&content, range);
        let start_line = range.map(|r| r.start).unwrap_or(1);
        let requested_end_line = range.and_then(|r| r.end);
        let total_file_lines = content.lines().count();
        let line_ending = detect_line_ending(&content);

        // Apply truncation
        let max_lines = ctx.read_max_lines;
        let result = if max_lines == 0 {
            super::TruncationResult {
                content: sliced.clone(),
                truncated: false,
                output_lines: sliced.lines().count(),
                total_lines: sliced.lines().count(),
                output_bytes: sliced.len(),
                total_bytes: sliced.len(),
                temp_file: None,
            }
        } else {
            truncate_head(&sliced, max_lines, MAX_BYTES)
        };

        let mut output = result.content.clone();
        let mut anchors_json = serde_json::Value::Null;
        if include_anchors {
            let visible_lines = result.content.lines().collect::<Vec<_>>();
            let anchors = ctx.anchor_store.record_lines(
                &path,
                super::stable_hash(&content),
                start_line,
                &visible_lines,
            );
            anchors_json = json!(anchors
                .iter()
                .map(|anchor| json!({
                    "line": anchor.line,
                    "anchor": anchor.id,
                    "content_hash": format!("{:016x}", anchor.content_hash),
                }))
                .collect::<Vec<_>>());
            if !anchors.is_empty() {
                output.push_str("\n\nAnchors:");
                for anchor in &anchors {
                    output.push_str(&format!("\n{:>6} {}", anchor.line, anchor.id));
                }
            }
        }
        if result.truncated {
            let note = format!(
                "\n[…truncated: showing {}/{} lines, {}/{} bytes",
                result.output_lines, result.total_lines, result.output_bytes, result.total_bytes,
            );
            if let Some(ref tf) = result.temp_file {
                output.push_str(&format!("{note}, full output: {}]", tf.display()));
            } else {
                output.push_str(&format!("{note}]"));
            }
        }

        // Record that this file was read (for staleness and unread-edit detection).
        if let Ok(mut tracker) = ctx.file_tracker.lock() {
            tracker.record_read(&path);
        }

        Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text { text: output }],
            details: json!({
                "action": "read",
                "path": path.display().to_string(),
                "start_line": start_line,
                "end_line": if result.output_lines == 0 { start_line.saturating_sub(1) } else { start_line + result.output_lines - 1 },
                "requested_end_line": requested_end_line,
                "truncated": result.truncated,
                "lines": result.output_lines,
                "total_lines": total_file_lines,
                "range_total_lines": result.total_lines,
                "lines_read": result.output_lines,
                "files": [{
                    "path": path.display().to_string(),
                    "status": "read",
                    "lines_read": result.output_lines,
                }],
                "bytes": result.output_bytes,
                "total_bytes": metadata.len(),
                "range_total_bytes": result.total_bytes,
                "temp_file": result.temp_file.as_ref().map(|path| path.display().to_string()),
                "encoding": "utf-8-lossy",
                "line_ending": line_ending,
                "anchors": anchors_json,
                "anchor_count": anchors_json.as_array().map(|anchors| anchors.len()).unwrap_or(0),
                "semantic_target": semantic_details,
            }),
            is_error: false,
        })
    }
}

#[derive(Clone, Copy)]
struct LineRange {
    start: usize,
    end: Option<usize>,
}

fn parse_line_target(target: &str) -> Option<(&str, usize)> {
    let (path, line) = target.rsplit_once(':')?;
    if path.is_empty() || line.is_empty() {
        return None;
    }
    let line = line.parse::<usize>().ok()?;
    (line > 0).then_some((path, line))
}

fn parse_line_range(params: &serde_json::Value) -> Result<Option<LineRange>> {
    let start_line = parse_positive_usize(params.get("start_line"), "start_line")?;
    let end_line = parse_positive_usize(params.get("end_line"), "end_line")?;

    if let (Some(start), Some(end)) = (start_line, end_line) {
        if start > end {
            return Err(crate::error::Error::Tool(
                "start_line must be <= end_line".to_string(),
            ));
        }
    }

    Ok(match (start_line, end_line) {
        (None, None) => None,
        (Some(start), end) => Some(LineRange { start, end }),
        (None, Some(end)) => Some(LineRange {
            start: 1,
            end: Some(end),
        }),
    })
}

fn parse_positive_usize(value: Option<&serde_json::Value>, field: &str) -> Result<Option<usize>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(number) = value.as_u64() else {
        return Err(crate::error::Error::Tool(format!(
            "{field} must be a positive integer"
        )));
    };
    if number == 0 {
        return Err(crate::error::Error::Tool(format!("{field} must be >= 1")));
    }
    Ok(Some(number as usize))
}

fn apply_line_range(content: &str, range: Option<LineRange>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = range
        .map(|range| range.start.saturating_sub(1))
        .unwrap_or(0);
    if start >= lines.len() {
        return String::new();
    }
    let end = range
        .and_then(|range| range.end)
        .map(|end| end.min(lines.len()))
        .unwrap_or(lines.len());

    lines[start..end].join("\n")
}

fn detect_line_ending(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "crlf"
    } else if content.contains('\r') {
        "cr"
    } else {
        "lf"
    }
}

async fn read_image(path: &Path) -> Result<ToolOutput> {
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.len() > MAX_IMAGE_BYTES {
        return Ok(ToolOutput::error(format!(
            "Image is too large to read: {} ({} bytes, max {} bytes)",
            path.display(),
            metadata.len(),
            MAX_IMAGE_BYTES
        )));
    }

    let bytes = tokio::fs::read(path).await?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();

    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };

    use std::io::Write;
    let mut encoded = Vec::new();
    {
        let mut encoder = base64_encoder(&mut encoded);
        encoder.write_all(&bytes)?;
        encoder.finish()?;
    }
    let data = String::from_utf8(encoded).unwrap_or_default();

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Image {
            media_type: media_type.to_string(),
            data,
        }],
        details: json!({
            "action": "read",
            "path": path.display().to_string(),
            "media_type": media_type,
            "bytes": bytes.len(),
            "total_bytes": metadata.len(),
        }),
        is_error: false,
    })
}

/// Simple base64 encoder without adding a dependency. We only need this for images.
fn base64_encoder(output: &mut Vec<u8>) -> Base64Writer<'_> {
    Base64Writer {
        output,
        buffer: [0; 3],
        buffer_len: 0,
    }
}

const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

struct Base64Writer<'a> {
    output: &'a mut Vec<u8>,
    buffer: [u8; 3],
    buffer_len: usize,
}

impl<'a> std::io::Write for Base64Writer<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &byte in buf {
            self.buffer[self.buffer_len] = byte;
            self.buffer_len += 1;
            if self.buffer_len == 3 {
                self.encode_block();
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> Base64Writer<'a> {
    fn encode_block(&mut self) {
        let b = &self.buffer;
        self.output.push(BASE64_CHARS[(b[0] >> 2) as usize]);
        self.output
            .push(BASE64_CHARS[((b[0] & 0x03) << 4 | b[1] >> 4) as usize]);
        self.output
            .push(BASE64_CHARS[((b[1] & 0x0f) << 2 | b[2] >> 6) as usize]);
        self.output.push(BASE64_CHARS[(b[2] & 0x3f) as usize]);
        self.buffer_len = 0;
    }

    fn finish(self) -> std::io::Result<()> {
        match self.buffer_len {
            1 => {
                let b = self.buffer[0];
                self.output.push(BASE64_CHARS[(b >> 2) as usize]);
                self.output.push(BASE64_CHARS[((b & 0x03) << 4) as usize]);
                self.output.push(b'=');
                self.output.push(b'=');
            }
            2 => {
                let b0 = self.buffer[0];
                let b1 = self.buffer[1];
                self.output.push(BASE64_CHARS[(b0 >> 2) as usize]);
                self.output
                    .push(BASE64_CHARS[((b0 & 0x03) << 4 | b1 >> 4) as usize]);
                self.output.push(BASE64_CHARS[((b1 & 0x0f) << 2) as usize]);
                self.output.push(b'=');
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
