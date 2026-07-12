use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use super::eval_runner;

#[derive(Subcommand, Debug)]
pub(crate) enum EvalCommand {
    /// List and validate task specs in an eval suite
    List(EvalListArgs),
    /// Validate one task spec or every task in a suite
    Validate(EvalValidateArgs),
    /// Prepare or execute one pinned eval task
    Run(Box<EvalRunArgs>),
    /// Compare two result.json files or result directories
    Compare(EvalCompareArgs),
}

#[derive(Args, Debug)]
pub(crate) struct EvalListArgs {
    /// Directory containing JSON task specs
    #[arg(long, default_value = "evals/coding-agent/tasks")]
    suite: PathBuf,
    /// Emit a JSON array
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub(crate) struct EvalValidateArgs {
    /// Task id or task JSON path; omit to validate the full suite
    task: Option<String>,
    /// Directory containing JSON task specs
    #[arg(long, default_value = "evals/coding-agent/tasks")]
    suite: PathBuf,
    /// Emit a JSON array
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub(crate) struct EvalRunArgs {
    /// Task id or task JSON path
    task: String,
    /// Directory containing JSON task specs
    #[arg(long, default_value = "evals/coding-agent/tasks")]
    suite: PathBuf,
    /// Result artifact directory
    #[arg(long, default_value = "evals/results")]
    results_dir: PathBuf,
    /// Isolated checkout directory
    #[arg(long, default_value_os_t = eval_runner::default_worktrees_dir())]
    worktrees_dir: PathBuf,
    /// Candidate agent implementation
    #[arg(long, value_enum, default_value_t = eval_runner::EvalAgentKind::Imp)]
    agent: eval_runner::EvalAgentKind,
    /// Candidate executable; defaults to the selected agent command from PATH
    #[arg(long, alias = "imp-binary")]
    agent_binary: Option<PathBuf>,
    /// Provider for the candidate run
    #[arg(long)]
    provider: Option<String>,
    /// Model for the candidate run
    #[arg(long)]
    model: Option<String>,
    /// Thinking level for the candidate run
    #[arg(long, default_value = "xhigh")]
    thinking: String,
    /// Compare this imp run against vanilla Pi
    #[arg(
        long,
        conflicts_with_all = ["prepare_only", "compare_against"]
    )]
    compare_pi: bool,
    /// Compare imp against one or more vanilla agents (comma-separated)
    #[arg(
        long,
        value_delimiter = ',',
        conflicts_with_all = ["prepare_only", "compare_pi"]
    )]
    compare_against: Vec<eval_runner::EvalAgentKind>,
    /// Pi executable used by comparisons; defaults to pi from PATH
    #[arg(long)]
    pi_binary: Option<PathBuf>,
    /// Codex executable used by comparisons; defaults to codex from PATH
    #[arg(long)]
    codex_binary: Option<PathBuf>,
    /// OpenCode executable used by comparisons; defaults to opencode from PATH
    #[arg(long)]
    opencode_binary: Option<PathBuf>,
    /// Repetitions for each agent in a comparison
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    /// Prepare the pinned checkout without running the agent or verifier
    #[arg(long)]
    prepare_only: bool,
    /// Emit the result object as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub(crate) struct EvalCompareArgs {
    /// Baseline result.json or its containing directory
    baseline: PathBuf,
    /// Candidate result.json or its containing directory
    candidate: PathBuf,
}

pub(crate) async fn run(command: &EvalCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        EvalCommand::List(args) => {
            let validations = validate_eval_paths(eval_runner::task_paths(&args.suite)?)?;
            print_eval_validations(&validations, args.json)?;
        }
        EvalCommand::Validate(args) => {
            let paths = match args.task.as_deref() {
                Some(task) => vec![eval_runner::resolve_task_path(&args.suite, task)],
                None => eval_runner::task_paths(&args.suite)?,
            };
            let validations = validate_eval_paths(paths)?;
            print_eval_validations(&validations, args.json)?;
            if validations.iter().any(|validation| !validation.valid) {
                return Err("one or more eval task specs are invalid".into());
            }
        }
        EvalCommand::Run(args) => {
            if args.compare_pi || !args.compare_against.is_empty() {
                run_comparison_eval_command(args).await?;
                return Ok(());
            }
            let task_path = eval_runner::resolve_task_path(&args.suite, &args.task);
            let agent_binary =
                eval_runner::resolve_binary(args.agent, args.agent_binary.as_deref())?;
            let options = eval_runner::EvalRunOptions {
                task_path,
                results_dir: args.results_dir.clone(),
                worktrees_dir: args.worktrees_dir.clone(),
                agent: args.agent,
                agent_binary,
                provider: args.provider.clone(),
                model: args.model.clone(),
                thinking: args.thinking.clone(),
                prepare_only: args.prepare_only,
            };
            let (result, output_dir) = eval_runner::run_eval(&options).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "{}\t{:?}\t{}",
                    result.task,
                    result.status,
                    output_dir.display()
                );
            }
            if !result.command_succeeded() {
                return Err(format!(
                    "eval task `{}` finished with status {:?}; artifacts: {}",
                    result.task,
                    result.status,
                    output_dir.display()
                )
                .into());
            }
        }
        EvalCommand::Compare(args) => {
            let baseline = eval_runner::load_result(&args.baseline)?;
            let candidate = eval_runner::load_result(&args.candidate)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&eval_runner::compare_results(
                    &baseline, &candidate
                )?)?
            );
        }
    }
    Ok(())
}

async fn run_comparison_eval_command(args: &EvalRunArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.agent != eval_runner::EvalAgentKind::Imp {
        return Err("comparisons require --agent imp".into());
    }
    let baselines = if args.compare_pi {
        vec![eval_runner::EvalAgentKind::Pi]
    } else {
        args.compare_against.clone()
    };
    if baselines.contains(&eval_runner::EvalAgentKind::Imp) {
        return Err("--compare-against cannot contain imp".into());
    }
    let provider = args.provider.as_deref().unwrap_or("openai-codex");
    let model = args.model.as_deref().unwrap_or("gpt-5.6-sol");
    let mut agents = vec![comparison_agent(
        eval_runner::EvalAgentKind::Imp,
        args.agent_binary.as_deref(),
    )?];
    for baseline in baselines {
        let override_path = match baseline {
            eval_runner::EvalAgentKind::Pi => args.pi_binary.as_deref(),
            eval_runner::EvalAgentKind::Codex => args.codex_binary.as_deref(),
            eval_runner::EvalAgentKind::Opencode => args.opencode_binary.as_deref(),
            eval_runner::EvalAgentKind::Imp => None,
        };
        agents.push(comparison_agent(baseline, override_path)?);
    }
    let report = eval_runner::run_comparison_eval(&eval_runner::ComparisonEvalOptions {
        task_path: eval_runner::resolve_task_path(&args.suite, &args.task),
        results_dir: args.results_dir.clone(),
        worktrees_dir: args.worktrees_dir.clone(),
        agents,
        provider: provider.to_string(),
        model: model.to_string(),
        thinking: args.thinking.clone(),
        repetitions: args.repeat,
    })
    .await?;
    print_comparison_eval_report(&report, args.json)
}

fn comparison_agent(
    kind: eval_runner::EvalAgentKind,
    override_path: Option<&Path>,
) -> Result<eval_runner::ComparisonAgent, Box<dyn std::error::Error>> {
    Ok(eval_runner::ComparisonAgent {
        kind,
        binary: eval_runner::resolve_binary(kind, override_path)?,
    })
}

fn print_comparison_eval_report(
    report: &eval_runner::ComparisonEvalReport,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!("task\t{}", report.task);
        println!("model\t{}/{}", report.provider, report.model);
        println!("repetitions\t{}", report.repetitions);
        for agent in &report.agents {
            println!(
                "{}\t{}/{} passed\tmedian {:?}ms",
                agent.agent, agent.passed, report.repetitions, agent.median_duration_ms
            );
        }
        println!("pass leaders\t{}", report.pass_leaders.join(","));
        println!("report\t{}", report.report_path.display());
    }
    Ok(())
}

fn validate_eval_paths(
    paths: Vec<PathBuf>,
) -> Result<Vec<eval_runner::SpecValidation>, Box<dyn std::error::Error>> {
    paths
        .into_iter()
        .map(|path| {
            let spec = eval_runner::EvalTaskSpec::load(&path)?;
            Ok(spec.validate(&path))
        })
        .collect()
}

fn print_eval_validations(
    validations: &[eval_runner::SpecValidation],
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(validations)?);
        return Ok(());
    }
    for validation in validations {
        let status = if validation.valid { "valid" } else { "invalid" };
        let detail = if validation.errors.is_empty() {
            String::new()
        } else {
            format!("\t{}", validation.errors.join("; "))
        };
        println!(
            "{}\t{}\t{}{}",
            validation.task,
            status,
            validation.path.display(),
            detail
        );
    }
    Ok(())
}
