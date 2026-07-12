use std::io::{self, Write};
use std::path::Path;

use imp_core::config::{Config, ToolOutputDisplay};
use imp_core::tools::web::types::SearchProvider;
use imp_llm::ThinkingLevel;

use crate::cli_config::{
    prompt_input_line, save_user_config, thinking_level_label, web_search_provider_label,
};

pub(crate) fn parse_thinking_level_strict(raw: &str) -> Option<ThinkingLevel> {
    match raw.trim().to_lowercase().as_str() {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::XHigh),
        _ => None,
    }
}

fn tool_output_display_label(display: ToolOutputDisplay) -> &'static str {
    match display {
        ToolOutputDisplay::Full => "full",
        ToolOutputDisplay::Compact => "compact",
        ToolOutputDisplay::Collapsed => "collapsed",
    }
}

pub(crate) fn parse_tool_output_display(raw: &str) -> Option<ToolOutputDisplay> {
    match raw.trim().to_lowercase().as_str() {
        "full" => Some(ToolOutputDisplay::Full),
        "compact" => Some(ToolOutputDisplay::Compact),
        "collapsed" => Some(ToolOutputDisplay::Collapsed),
        _ => None,
    }
}

fn prompt_optional_input_line(prompt: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    eprint!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    let bytes = io::stdin().read_line(&mut input)?;
    if bytes == 0 {
        return Ok(None);
    }
    Ok(Some(input.trim().to_string()))
}

fn print_settings_summary(config: &Config, config_path: &Path) {
    println!("Settings ({})", config_path.display());
    println!("================");
    println!(
        "1. model               {}",
        config.model.as_deref().unwrap_or("(unset)")
    );
    println!(
        "2. thinking            {}",
        config
            .thinking
            .map(thinking_level_label)
            .unwrap_or("(unset)")
    );
    println!(
        "3. max_tokens          {}",
        config
            .max_tokens
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(unset)".to_string())
    );
    println!(
        "4. max_turns           {}",
        config
            .max_turns
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(unset)".to_string())
    );
    println!(
        "5. tool_output         {}",
        tool_output_display_label(config.ui.tool_output)
    );
    println!(
        "6. web_search_provider {}",
        web_search_provider_label(config.web.search_provider)
    );
    println!("s. save and exit");
    println!("q. quit without saving");
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let config_path = imp_core::storage::global_config_path();
    let mut config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;

    loop {
        println!();
        print_settings_summary(&config, &config_path);
        let Some(choice) = prompt_optional_input_line("Select field> ")? else {
            println!();
            return Ok(());
        };

        match choice.trim() {
            "1" => {
                let value = prompt_input_line("model> ")?;
                if value.is_empty() {
                    config.model = None;
                    println!("Cleared model.");
                } else {
                    config.model = Some(value);
                    println!("Updated model.");
                }
            }
            "2" => {
                let value = prompt_input_line("thinking [off|minimal|low|medium|high|xhigh]> ")?;
                if value.is_empty() {
                    config.thinking = None;
                    println!("Cleared thinking level.");
                } else if let Some(level) = parse_thinking_level_strict(&value) {
                    config.thinking = Some(level);
                    println!("Updated thinking level.");
                } else {
                    println!("Unknown thinking level: {value}");
                }
            }
            "3" => {
                let value = prompt_input_line("max_tokens> ")?;
                if value.is_empty() {
                    config.max_tokens = None;
                    println!("Cleared max_tokens.");
                } else if let Ok(parsed) = value.parse::<u32>() {
                    config.max_tokens = Some(parsed.max(1));
                    println!("Updated max_tokens.");
                } else {
                    println!("Expected a positive integer.");
                }
            }
            "4" => {
                let value = prompt_input_line("max_turns> ")?;
                if value.is_empty() {
                    config.max_turns = None;
                    println!("Cleared max_turns.");
                } else if let Ok(parsed) = value.parse::<u32>() {
                    config.max_turns = Some(parsed.max(1));
                    println!("Updated max_turns.");
                } else {
                    println!("Expected a positive integer.");
                }
            }
            "5" => {
                let value = prompt_input_line("tool_output [full|compact|collapsed]> ")?;
                if let Some(display) = parse_tool_output_display(&value) {
                    config.ui.tool_output = display;
                    println!("Updated tool output display.");
                } else {
                    println!("Expected one of: full, compact, collapsed.");
                }
            }
            "6" => {
                let value =
                    prompt_input_line("web_search_provider [none|tavily|exa|linkup|perplexity]> ")?;
                match value.trim().to_lowercase().as_str() {
                    "" | "none" => {
                        config.web.search_provider = None;
                        println!("Cleared web search provider.");
                    }
                    "tavily" => {
                        config.web.search_provider = Some(SearchProvider::Tavily);
                        println!("Updated web search provider.");
                    }
                    "exa" => {
                        config.web.search_provider = Some(SearchProvider::Exa);
                        println!("Updated web search provider.");
                    }
                    "linkup" => {
                        config.web.search_provider = Some(SearchProvider::Linkup);
                        println!("Updated web search provider.");
                    }
                    "perplexity" => {
                        config.web.search_provider = Some(SearchProvider::Perplexity);
                        println!("Updated web search provider.");
                    }
                    _ => println!("Expected one of: none, tavily, exa, linkup, perplexity."),
                }
            }
            "s" | "save" => {
                let saved_path = save_user_config(&config)?;
                println!("Saved settings to {}", saved_path.display());
                return Ok(());
            }
            "q" | "quit" => {
                println!("Discarded settings changes.");
                return Ok(());
            }
            other => {
                println!("Unknown selection: {other}");
            }
        }
    }
}
