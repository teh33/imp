use std::collections::HashMap;
use std::io::{self, Write};

use imp_core::tools::web::types::SearchProvider;
use imp_llm::auth::AuthStore;

pub(crate) fn canonical_provider_name(name: &str) -> String {
    provider_alias(name)
}

pub(crate) fn resolve_stored_provider_name(auth_store: &AuthStore, name: &str) -> Option<String> {
    let canonical = canonical_provider_name(name);
    if auth_store.stored.contains_key(&canonical) {
        return Some(canonical);
    }

    auth_store
        .stored
        .keys()
        .find(|stored| stored.eq_ignore_ascii_case(&canonical))
        .cloned()
}

pub(crate) fn provider_alias(name: &str) -> String {
    match name.trim().to_lowercase().as_str() {
        "kimi" => "moonshot".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn search_provider_from_name(name: &str) -> Option<SearchProvider> {
    match name.trim().to_lowercase().as_str() {
        "tavily" => Some(SearchProvider::Tavily),
        "exa" => Some(SearchProvider::Exa),
        "linkup" => Some(SearchProvider::Linkup),
        "perplexity" => Some(SearchProvider::Perplexity),
        "github" => Some(SearchProvider::GitHub),
        _ => None,
    }
}

pub(crate) fn search_provider_docs_url(provider: SearchProvider) -> &'static str {
    match provider {
        SearchProvider::Tavily => "https://app.tavily.com/home",
        SearchProvider::Exa => "https://dashboard.exa.ai/api-keys",
        SearchProvider::Linkup => "https://app.linkup.so/api-keys",
        SearchProvider::Perplexity => "https://www.perplexity.ai/settings/api",
        SearchProvider::GitHub => "https://github.com/settings/tokens",
    }
}

fn parse_secret_field_names(input: &str) -> Vec<String> {
    let names: Vec<String> = input
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .collect();
    if names.is_empty() {
        vec!["api_key".to_string()]
    } else {
        names
    }
}

pub(crate) fn prompt_for_secret_fields(
    _provider_name: &str,
    display_name: &str,
    docs_hint: &str,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    if docs_hint.is_empty() {
        eprintln!("Saving credentials for {display_name}.");
    } else {
        eprintln!("Saving credentials for {display_name}. Get them at: {docs_hint}");
    }

    eprintln!("Field names (comma-separated) [api_key]:");
    eprint!("> ");
    io::stdout().flush().ok();
    let mut field_input = String::new();
    std::io::stdin().read_line(&mut field_input)?;
    let field_names = parse_secret_field_names(&field_input);

    let mut fields = HashMap::new();
    for field in field_names {
        eprintln!("Enter {field}:");
        eprint!("> ");
        io::stdout().flush().ok();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let value = input.trim().to_string();
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("No value entered for {field}. Aborting."),
            )
            .into());
        }
        fields.insert(field, value);
    }

    Ok(fields)
}
