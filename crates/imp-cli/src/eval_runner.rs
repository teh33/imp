mod agent;
mod checkout;
mod diff;
mod evaluation;
mod paired;
mod process;
mod result;
mod spec;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

pub(crate) use agent::{resolve_binary, EvalAgentKind};
pub(crate) use paired::{
    run_comparison_eval, ComparisonAgent, ComparisonEvalOptions, ComparisonEvalReport,
};
pub(crate) use result::{compare_results, load_result, EvalRunResult};
pub(crate) use spec::{resolve_task_path, task_paths, EvalTaskSpec, SpecValidation};

use checkout::{
    commit_setup_baseline, configure_eval_excludes, prepare_checkout, FixtureSourceGuard,
};
use diff::capture_diff;
use evaluation::{
    evaluate_expectations, expand_verifier, format_agent_failure, format_process_failure, run_id,
    run_shell_to_file,
};
use process::run_logged_with_env;
use result::{write_result, EvalRunStatus};

const DEFAULT_AGENT_TIMEOUT_SECONDS: u64 = 30 * 60;
const DEFAULT_VERIFIER_TIMEOUT_SECONDS: u64 = 15 * 60;

pub(crate) fn default_worktrees_dir() -> PathBuf {
    std::env::temp_dir().join("imp-eval-worktrees")
}

#[derive(Debug, Clone)]
pub(crate) struct EvalRunOptions {
    pub task_path: PathBuf,
    pub results_dir: PathBuf,
    pub worktrees_dir: PathBuf,
    pub agent: EvalAgentKind,
    pub agent_binary: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: String,
    pub prepare_only: bool,
}

fn set_agent_error(result: &mut result::EvalAgentResult, message: &str) {
    let outcome = result.outcome.get_or_insert_with(|| serde_json::json!({}));
    outcome["status"] = serde_json::json!("error");
    outcome["error"] = serde_json::json!(message);
}

pub(crate) async fn run_eval(
    options: &EvalRunOptions,
) -> Result<(EvalRunResult, PathBuf), Box<dyn std::error::Error>> {
    let spec = EvalTaskSpec::load(&options.task_path)?;
    let validation = spec.validate(&options.task_path);
    if !validation.valid {
        return Err(format!(
            "eval task `{}` is invalid: {}",
            spec.id,
            validation.errors.join("; ")
        )
        .into());
    }

    let run_id = run_id(&spec.id);
    let output_dir = options.results_dir.join(&run_id);
    let checkout = options.worktrees_dir.join(&spec.id);
    fs::create_dir_all(&output_dir)?;
    fs::create_dir_all(&options.worktrees_dir)?;
    fs::write(output_dir.join("prompt.md"), spec.execution_prompt())?;

    let fixture_guard = FixtureSourceGuard::capture(&spec, &options.task_path)?;
    let mut result = EvalRunResult::initial(run_id, &spec, checkout.clone());
    let base_commit = prepare_checkout(&spec, &options.task_path, &checkout).await?;
    let mut evaluation_base = base_commit.clone();
    configure_eval_excludes(&checkout)?;
    result.source.commit = base_commit.clone();
    if let Some(setup) = spec.setup.as_deref() {
        let setup_outcome = run_shell_to_file(
            setup,
            &checkout,
            &output_dir.join("setup.txt"),
            Duration::from_secs(
                spec.timeout_seconds
                    .unwrap_or(DEFAULT_AGENT_TIMEOUT_SECONDS),
            ),
        )
        .await?;
        if !setup_outcome.success() {
            result.error = Some(format_process_failure("setup", setup_outcome));
            write_result(&output_dir, &result)?;
            return Ok((result, output_dir));
        }
        evaluation_base = commit_setup_baseline(&checkout).await?;
    }

    if options.prepare_only {
        result.status = EvalRunStatus::Prepared;
        write_result(&output_dir, &result)?;
        return Ok((result, output_dir));
    }

    let provider = options
        .provider
        .as_deref()
        .ok_or("--provider is required unless --prepare-only is used")?;
    let model = options
        .model
        .as_deref()
        .ok_or("--model is required unless --prepare-only is used")?;
    if !options.agent_binary.is_file() && options.agent_binary.components().count() > 1 {
        return Err(format!(
            "{} binary does not exist: {}",
            options.agent.name(),
            options.agent_binary.display()
        )
        .into());
    }

    result.agent.kind = Some(options.agent.name().to_string());
    result.agent.provider = Some(provider.to_string());
    result.agent.model = Some(model.to_string());
    result.agent.thinking = Some(options.thinking.clone());
    let invocation =
        options
            .agent
            .invocation(&spec, provider, model, &options.thinking, &checkout)?;
    let agent_outcome = run_logged_with_env(
        &options.agent_binary,
        &invocation.args,
        &invocation.env,
        &checkout,
        &output_dir.join("agent.stdout.json"),
        &output_dir.join("agent.stderr.txt"),
        Duration::from_secs(
            spec.timeout_seconds
                .unwrap_or(DEFAULT_AGENT_TIMEOUT_SECONDS),
        ),
    )
    .await?;
    let mut agent_succeeded = options.agent.record_outcome(
        &mut result.agent,
        agent_outcome,
        &output_dir.join("agent.stdout.json"),
    );
    let source_mutated = match &fixture_guard {
        Some(guard) => guard.restore_if_changed(&output_dir.join("fixture-source-violation"))?,
        None => false,
    };
    if source_mutated {
        set_agent_error(
            &mut result.agent,
            "agent modified the immutable fixture source outside its checkout",
        );
        agent_succeeded = false;
    }

    let (patch, diff_summary) = capture_diff(&checkout, &output_dir, &evaluation_base).await?;
    fs::write(output_dir.join("diff.patch"), patch)?;
    result.diff = diff_summary;
    result.expectations = evaluate_expectations(&spec, &result.diff);

    if spec.verifier.contains("<modified-files>") && result.diff.paths.is_empty() {
        result.error = Some(if agent_succeeded {
            "verifier requires modified files, but the agent produced no tracked diff".to_string()
        } else {
            format_agent_failure(&result.agent, agent_outcome)
        });
        result.status = EvalRunStatus::Failed;
        fs::write(
            output_dir.join("verifier.txt"),
            "Verifier was not run because <modified-files> expanded to an empty set.\n",
        )?;
        write_result(&output_dir, &result)?;
        return Ok((result, output_dir));
    }

    let verifier = expand_verifier(&spec.verifier, &result.diff.paths);
    result.verifier.command = verifier.clone();
    let verifier_outcome = run_shell_to_file(
        &verifier,
        &checkout,
        &output_dir.join("verifier.txt"),
        Duration::from_secs(
            spec.verifier_timeout_seconds
                .unwrap_or(DEFAULT_VERIFIER_TIMEOUT_SECONDS),
        ),
    )
    .await?;
    result.verifier.exit_code = verifier_outcome.exit_code;
    result.verifier.timed_out = verifier_outcome.timed_out;
    result.verifier.duration_ms = Some(verifier_outcome.duration_ms);
    result.verifier.passed = Some(verifier_outcome.success());

    result.status = if agent_succeeded && verifier_outcome.success() && result.expectations.passed {
        EvalRunStatus::Passed
    } else {
        result.error = Some(format!(
            "agent: {}; verifier: {}; expectations: {}",
            if agent_succeeded {
                "passed".to_string()
            } else {
                format_agent_failure(&result.agent, agent_outcome)
            },
            if verifier_outcome.success() {
                "passed".to_string()
            } else {
                format_process_failure("verifier", verifier_outcome)
            },
            if result.expectations.passed {
                "passed".to_string()
            } else {
                result.expectations.failures.join("; ")
            }
        ));
        EvalRunStatus::Failed
    };
    write_result(&output_dir, &result)?;
    Ok((result, output_dir))
}
