pub(crate) fn expand_file_args(args: &[String]) -> String {
    let mut parts = Vec::new();
    for arg in args {
        if let Some(path_str) = arg.strip_prefix('@') {
            let path = std::path::Path::new(path_str);
            // Expand ~ in @~/path
            let resolved = if let Some(rest) = path_str.strip_prefix("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    std::path::PathBuf::from(home).join(rest)
                } else {
                    path.to_path_buf()
                }
            } else {
                path.to_path_buf()
            };
            match std::fs::read_to_string(&resolved) {
                Ok(content) => {
                    parts.push(format!(
                        "<file path=\"{}\">\n{}\n</file>",
                        resolved.display(),
                        content.trim_end()
                    ));
                }
                Err(e) => {
                    eprintln!("Warning: cannot read {}: {e}", resolved.display());
                }
            }
        }
    }
    parts.join("\n\n")
}

pub(crate) fn prompt_args(args: &[String]) -> Vec<&str> {
    args.iter()
        .filter(|arg| !arg.starts_with('@'))
        .map(String::as_str)
        .collect()
}

pub(crate) fn build_full_prompt(
    prompt: &str,
    file_context: &str,
    stdin: &Option<String>,
) -> String {
    let mut parts = Vec::new();
    if !file_context.is_empty() {
        parts.push(file_context.to_string());
    }
    if let Some(ref content) = stdin {
        parts.push(format!("<stdin>\n{}\n</stdin>", content.trim_end()));
    }
    if !prompt.is_empty() {
        parts.push(prompt.to_string());
    }
    parts.join("\n\n")
}
