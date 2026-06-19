use std::io::{self, Write};
use std::path::PathBuf;

use imp_core::config::Config;
use imp_core::tools::web::types::SearchProvider;
use imp_llm::ThinkingLevel;

pub(crate) fn thinking_level_label(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhigh",
    }
}

pub(crate) fn web_search_provider_label(provider: Option<SearchProvider>) -> &'static str {
    match provider {
        Some(provider) => provider.name(),
        None => "none",
    }
}

pub(crate) fn prompt_input_line(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    eprint!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    let bytes = io::stdin().read_line(&mut input)?;
    if bytes == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed").into());
    }
    Ok(input.trim().to_string())
}

pub(crate) fn save_user_config(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = imp_core::storage::global_config_path();
    config
        .save(&path)
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    Ok(path)
}
