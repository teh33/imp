use std::io;

use clap::Subcommand;
use imp_llm::auth::{AuthStore, SecretFieldStatus, SecretStatus, StoredCredential};
use imp_llm::model::ProviderRegistry;

use crate::provider_secrets::{
    canonical_provider_name, prompt_for_secret_fields, provider_alias, resolve_stored_provider_name,
};

#[derive(Subcommand, Debug)]
pub(crate) enum SecretsCommand {
    /// List configured secret providers/services
    List,
    /// Alias for list
    Ls,
    /// Show status for one configured provider/service
    Show {
        /// Provider/service to inspect
        provider: String,
    },
    /// Alias for show
    Inspect {
        /// Provider/service to inspect
        provider: String,
    },
    /// Verify that configured secrets are readable from secure storage
    Doctor,
    /// Remove a configured provider/service from secure storage
    Remove {
        /// Provider/service to remove
        provider: String,
    },
    /// Alias for remove
    Rm {
        /// Provider/service to remove
        provider: String,
    },
    /// Configure or update a provider/service's secret fields
    Set {
        /// Provider/service to configure (e.g. tavily, exa, resend, my-service)
        provider: String,
    },
}

pub(crate) async fn run_command(
    command: Option<&SecretsCommand>,
    provider: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Some(SecretsCommand::List) | Some(SecretsCommand::Ls) => run_secrets_list(),
        Some(SecretsCommand::Show { provider }) | Some(SecretsCommand::Inspect { provider }) => {
            run_secrets_show(provider)
        }
        Some(SecretsCommand::Remove { provider }) | Some(SecretsCommand::Rm { provider }) => {
            run_secrets_remove(provider)
        }
        Some(SecretsCommand::Doctor) => run_secrets_doctor(),
        Some(SecretsCommand::Set { provider }) => run_secrets_login(provider).await,
        None => {
            let provider = provider.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Usage: imp secrets <provider> | imp secrets list | imp secrets show <provider> | imp secrets rm <provider>",
                )
            })?;
            run_secrets_login(provider).await
        }
    }
}

#[derive(Debug)]
struct SecretListRow {
    id: String,
    display_name: String,
    kind: String,
    fields: String,
    status: String,
}

fn secret_status_label(status: Option<&SecretStatus>) -> String {
    let Some(status) = status else {
        return "unknown".to_string();
    };
    if status.is_usable() {
        return "ok".to_string();
    }

    let broken_fields: Vec<String> = status
        .fields
        .iter()
        .filter_map(|(field, field_status)| match field_status {
            SecretFieldStatus::Present => None,
            SecretFieldStatus::Missing => Some(format!("{field}:missing")),
            SecretFieldStatus::Error(_) => Some(format!("{field}:error")),
        })
        .collect();

    if broken_fields.is_empty() {
        "broken".to_string()
    } else {
        format!("broken ({})", broken_fields.join(", "))
    }
}

fn secret_kind_and_fields(entry: &StoredCredential) -> (String, String) {
    match entry {
        StoredCredential::OAuth(_) => ("oauth".to_string(), "access_token".to_string()),
        StoredCredential::ApiKey { .. } => ("api_key".to_string(), "api_key".to_string()),
        StoredCredential::SecretFields { fields } => {
            let kind = if fields.len() == 1 && fields.first().map(String::as_str) == Some("api_key")
            {
                "api_key".to_string()
            } else {
                format!("{} fields", fields.len())
            };
            (kind, fields.join(", "))
        }
    }
}

fn secret_status_detail(status: &SecretStatus) -> String {
    status
        .fields
        .iter()
        .map(|(field, field_status)| match field_status {
            SecretFieldStatus::Present => format!("{field}: ok"),
            SecretFieldStatus::Missing => format!("{field}: missing from secure storage"),
            SecretFieldStatus::Error(error) => format!("{field}: secure storage error: {error}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_secrets_list() -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));

    if auth_store.stored.is_empty() {
        println!("No saved credentials.");
        return Ok(());
    }

    let registry = ProviderRegistry::with_builtins();
    let mut rows: Vec<SecretListRow> = auth_store
        .stored
        .iter()
        .map(|(name, entry)| {
            let display_name = registry
                .find(name)
                .map(|meta| meta.name.to_string())
                .unwrap_or_else(|| name.clone());
            let (kind, fields) = secret_kind_and_fields(entry);
            let status = secret_status_label(auth_store.secret_status(name).as_ref());
            SecretListRow {
                id: name.clone(),
                display_name,
                kind,
                fields,
                status,
            }
        })
        .collect();

    rows.sort_by(|a, b| a.id.cmp(&b.id));

    let provider_w = rows
        .iter()
        .map(|row| format!("{} ({})", row.display_name, row.id).len())
        .max()
        .unwrap_or(8)
        .max("Provider".len());
    let kind_w = rows
        .iter()
        .map(|row| row.kind.len())
        .max()
        .unwrap_or(4)
        .max("Kind".len());
    let status_w = rows
        .iter()
        .map(|row| row.status.len())
        .max()
        .unwrap_or(6)
        .max("Status".len());

    println!(
        "{:<provider_w$}  {:<kind_w$}  {:<status_w$}  Fields",
        "Provider",
        "Kind",
        "Status",
        provider_w = provider_w,
        kind_w = kind_w,
        status_w = status_w
    );
    println!(
        "{:-<provider_w$}  {:-<kind_w$}  {:-<status_w$}  {:-<6}",
        "",
        "",
        "",
        "",
        provider_w = provider_w,
        kind_w = kind_w,
        status_w = status_w
    );

    let has_broken = rows.iter().any(|row| row.status != "ok");
    for row in rows {
        println!(
            "{:<provider_w$}  {:<kind_w$}  {:<status_w$}  {}",
            format!("{} ({})", row.display_name, row.id),
            row.kind,
            row.status,
            row.fields,
            provider_w = provider_w,
            kind_w = kind_w,
            status_w = status_w
        );
    }

    if has_broken {
        eprintln!(
            "\nSome secret metadata points at missing secure-storage values. Re-save with `imp secrets <provider>` or run `imp secrets doctor` for details."
        );
    }

    Ok(())
}

fn run_secrets_show(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let requested_provider = canonical_provider_name(provider);
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
    let registry = ProviderRegistry::with_builtins();

    let provider =
        resolve_stored_provider_name(&auth_store, &requested_provider).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("No saved credentials for {requested_provider}."),
            )
        })?;
    let entry = auth_store.stored.get(&provider).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("No saved credentials for {provider}."),
        )
    })?;

    let display_name = registry
        .find(&provider)
        .map(|meta| meta.name.to_string())
        .unwrap_or_else(|| provider.to_string());

    let (kind, fields) = secret_kind_and_fields(entry);
    let status = auth_store.secret_status(&provider).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("No saved credentials for {provider}."),
        )
    })?;

    println!("Provider : {} ({})", display_name, provider);
    println!("Kind     : {}", kind);
    println!("Fields   : {}", fields);
    println!("Storage  : secure keychain + auth metadata");
    println!("Status   : {}", secret_status_label(Some(&status)));
    println!("Values   : hidden");
    println!("\n{}", secret_status_detail(&status));

    if !status.is_usable() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "Saved metadata for {provider} exists, but one or more secret values are missing or unreadable. Re-save with `imp secrets {provider}`."
            ),
        )
        .into());
    }

    Ok(())
}

fn run_secrets_doctor() -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));

    if auth_store.stored.is_empty() {
        println!("No saved credentials.");
        return Ok(());
    }

    let registry = ProviderRegistry::with_builtins();
    let mut providers: Vec<_> = auth_store.stored.keys().cloned().collect();
    providers.sort();

    let mut broken = Vec::new();
    for provider in providers {
        let display_name = registry
            .find(&provider)
            .map(|meta| meta.name.to_string())
            .unwrap_or_else(|| provider.clone());
        let status = auth_store.secret_status(&provider).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("No saved credentials for {provider}."),
            )
        })?;
        println!(
            "{} ({}) — {}",
            display_name,
            provider,
            secret_status_label(Some(&status))
        );
        for line in secret_status_detail(&status).lines() {
            println!("  {line}");
        }
        if !status.is_usable() {
            broken.push(provider);
        }
    }

    if broken.is_empty() {
        return Ok(());
    }

    eprintln!(
        "\nBroken secrets: {}. Re-save each with `imp secrets <provider>`; metadata without keychain values cannot authenticate.",
        broken.join(", ")
    );
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} saved secret provider(s) are not usable", broken.len()),
    )
    .into())
}

fn run_secrets_remove(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let provider = provider_alias(provider);
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
    auth_store.remove(&provider)?;
    eprintln!("Removed saved credentials for {provider}.");
    Ok(())
}

async fn run_secrets_login(provider_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

    let canonical_provider = provider_alias(provider_name);
    let registry = ProviderRegistry::with_builtins();
    let provider_meta = registry.find(&canonical_provider);
    let display_name = provider_meta.map(|p| p.name).unwrap_or(&canonical_provider);
    let docs_hint = provider_meta.map(|p| p.docs_url).unwrap_or("");

    let fields = prompt_for_secret_fields(&canonical_provider, display_name, docs_hint)?;
    auth_store.store_secret_fields(&canonical_provider, fields)?;
    eprintln!("Credentials saved for {display_name}.");
    Ok(())
}
