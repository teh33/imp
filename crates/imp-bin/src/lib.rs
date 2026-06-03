use imp_cli::{disposition, parse_cli, parse_thinking_level, Cli, CliRunDisposition};
use imp_core::config::Config;
use imp_core::session::SessionManager;
use imp_llm::model::ModelRegistry;

pub async fn run() {
    let cli = parse_cli();
    match disposition(&cli) {
        CliRunDisposition::Headless => imp_cli::run_headless(cli).await,
        CliRunDisposition::Tui => {
            if let Err(error) = run_interactive(&cli).await {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
    }
}

async fn run_interactive(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;
    let registry = ModelRegistry::with_builtins();

    let session = if cli.no_session() {
        SessionManager::in_memory()
    } else if cli.continue_recent() {
        match SessionManager::continue_recent(&cwd, &imp_core::storage::global_sessions_dir())? {
            Some(session) => session,
            None => SessionManager::new(&cwd, &imp_core::storage::global_sessions_dir())?,
        }
    } else if let Some(path) = cli.session_path() {
        SessionManager::open(path)?
    } else {
        SessionManager::new(&cwd, &imp_core::storage::global_sessions_dir())?
    };

    let mut runner = imp_tui::interactive::InteractiveRunner::new(config, session, registry, cwd)?;

    if let Some(model) = cli.model_override() {
        runner.app_mut().model_name = model.to_string();
    }
    if let Some(thinking) = cli.thinking_override() {
        runner.app_mut().thinking_level = parse_thinking_level(thinking);
    }

    runner.run_guarded().await.map_err(Into::into)
}
