use std::path::Path;

use serde_json::json;

use super::super::ToolOutput;
use crate::tools::code_intel;

pub(super) struct ExactOutput<'a> {
    pub message: String,
    pub path: &'a Path,
    pub dry_run: bool,
    pub replace_all: bool,
    pub exact_occurrences: usize,
    pub replacements: usize,
    pub target: Option<&'a str>,
    pub syntax_validation: Option<code_intel::SyntaxValidation>,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub symbol_diff: code_intel::SymbolDiff,
    pub was_fuzzy: bool,
}

impl ExactOutput<'_> {
    pub(super) fn into_tool_output(self) -> ToolOutput {
        ToolOutput {
            content: vec![imp_llm::ContentBlock::Text { text: self.message }],
            details: json!({
                "action": "edit", "mode": "single",
                "path": self.path.display().to_string(), "fuzzy_match": self.was_fuzzy,
                "dry_run": self.dry_run, "replace_all": self.replace_all,
                "exact_occurrences": self.exact_occurrences, "replacements": self.replacements,
                "target": self.target,
                "syntax_validation": self.syntax_validation.as_ref().map(|validation| json!({
                    "supported": validation.supported, "valid": validation.valid,
                    "language": validation.language,
                    "errors": validation.errors.iter().map(|error| json!({
                        "start_line": error.start_line,
                        "end_line": error.end_line,
                        "kind": error.kind,
                    })).collect::<Vec<_>>(),
                })),
                "lines_added": self.lines_added, "lines_removed": self.lines_removed,
                "files": [{
                    "path": self.path.display().to_string(), "status": "modified",
                    "lines_added": self.lines_added, "lines_removed": self.lines_removed,
                }],
                "changed_symbols": {
                    "added": self.symbol_diff.added.iter().cloned().collect::<Vec<_>>(),
                    "removed": self.symbol_diff.removed.iter().cloned().collect::<Vec<_>>(),
                },
            }),
            is_error: false,
        }
    }
}
