mod report;
mod usage;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::spec::EvalTaskSpec;
use super::{run_eval, EvalAgentKind, EvalRunOptions};
use report::{aggregate, pass_leaders, summarize, AgentRun, ComparisonRun};
pub(crate) use report::{AgentAggregate, ComparisonEvalReport};

const MAX_REPETITIONS: u32 = 20;

#[derive(Debug, Clone)]
pub(crate) struct ComparisonEvalOptions {
    pub task_path: PathBuf,
    pub results_dir: PathBuf,
    pub worktrees_dir: PathBuf,
    pub agents: Vec<ComparisonAgent>,
    pub provider: String,
    pub model: String,
    pub thinking: String,
    pub repetitions: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct ComparisonAgent {
    pub kind: EvalAgentKind,
    pub binary: PathBuf,
}

pub(crate) async fn run_comparison_eval(
    options: &ComparisonEvalOptions,
) -> Result<ComparisonEvalReport, Box<dyn std::error::Error>> {
    validate_options(options)?;
    let comparison_id = comparison_id(&options.agents);
    let task = EvalTaskSpec::load(&options.task_path)?.id;
    let runs = execute_repetitions(options, &comparison_id, &task).await?;
    let agents = aggregate_agents(&runs);
    persist_report(options, comparison_id, task, agents, runs)
}

async fn execute_repetitions(
    options: &ComparisonEvalOptions,
    comparison_id: &str,
    task: &str,
) -> Result<Vec<ComparisonRun>, Box<dyn std::error::Error>> {
    let comparison_worktrees = options.worktrees_dir.join(comparison_id);
    let initial_offset = task_offset(task, options.agents.len());
    let mut runs = Vec::with_capacity(options.repetitions as usize);
    for repetition in 1..=options.repetitions {
        let order = rotated_agents(&options.agents, initial_offset + repetition as usize - 1);
        eprintln!(
            "comparison {repetition}/{}: {}",
            options.repetitions,
            order
                .iter()
                .map(|agent| agent.kind.name())
                .collect::<Vec<_>>()
                .join(" → ")
        );
        let mut agent_runs = Vec::with_capacity(order.len());
        for agent in &order {
            let (result, artifact_dir) = run_agent(options, &comparison_worktrees, agent).await?;
            agent_runs.push(AgentRun {
                agent: agent.kind.name().to_string(),
                summary: summarize(result, artifact_dir),
            });
        }
        agent_runs.sort_by(|left, right| left.agent.cmp(&right.agent));
        runs.push(ComparisonRun {
            repetition,
            execution_order: order
                .iter()
                .map(|agent| agent.kind.name().to_string())
                .collect(),
            agents: agent_runs,
        });
    }

    Ok(runs)
}

fn persist_report(
    options: &ComparisonEvalOptions,
    comparison_id: String,
    task: String,
    agents: Vec<AgentAggregate>,
    runs: Vec<ComparisonRun>,
) -> Result<ComparisonEvalReport, Box<dyn std::error::Error>> {
    let report_path = options
        .results_dir
        .join("comparisons")
        .join(format!("{comparison_id}.json"));
    let report = ComparisonEvalReport {
        schema_version: 2,
        comparison_id,
        task,
        provider: options.provider.clone(),
        model: options.model.clone(),
        thinking: options.thinking.clone(),
        repetitions: options.repetitions,
        pass_leaders: pass_leaders(&agents),
        agents,
        runs,
        report_path: report_path.clone(),
    };
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &report_path,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    Ok(report)
}

fn validate_options(options: &ComparisonEvalOptions) -> Result<(), Box<dyn std::error::Error>> {
    if options.repetitions == 0 || options.repetitions > MAX_REPETITIONS {
        return Err(format!("--repeat must be between 1 and {MAX_REPETITIONS}").into());
    }
    if options.agents.len() < 2 {
        return Err("comparison eval requires at least two agents".into());
    }
    if options.agents.first().map(|agent| agent.kind) != Some(EvalAgentKind::Imp) {
        return Err("comparison eval must use imp as the candidate agent".into());
    }
    let mut names = options
        .agents
        .iter()
        .map(|agent| agent.kind.name())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    if names.len() != options.agents.len() {
        return Err("comparison eval agent list contains duplicates".into());
    }
    Ok(())
}

fn task_offset(task: &str, agent_count: usize) -> usize {
    let hash = task.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    ((hash ^ (hash >> 16)) as usize) % agent_count
}

fn rotated_agents(agents: &[ComparisonAgent], offset: usize) -> Vec<ComparisonAgent> {
    let mut rotated = agents.to_vec();
    let length = rotated.len();
    rotated.rotate_left(offset % length);
    rotated
}

async fn run_agent(
    options: &ComparisonEvalOptions,
    comparison_worktrees: &std::path::Path,
    agent: &ComparisonAgent,
) -> Result<(super::EvalRunResult, PathBuf), Box<dyn std::error::Error>> {
    run_eval(&EvalRunOptions {
        task_path: options.task_path.clone(),
        results_dir: options.results_dir.clone(),
        worktrees_dir: comparison_worktrees.join(agent.kind.name()),
        agent: agent.kind,
        agent_binary: agent.binary.clone(),
        provider: Some(options.provider.clone()),
        model: Some(options.model.clone()),
        thinking: options.thinking.clone(),
        prepare_only: false,
    })
    .await
}

fn aggregate_agents(runs: &[ComparisonRun]) -> Vec<AgentAggregate> {
    let mut grouped = BTreeMap::<&str, Vec<&report::RunSummary>>::new();
    for run in runs {
        for agent in &run.agents {
            grouped
                .entry(&agent.agent)
                .or_default()
                .push(&agent.summary);
        }
    }
    grouped
        .into_iter()
        .map(|(agent, summaries)| aggregate(agent, &summaries))
        .collect()
}

fn comparison_id(agents: &[ComparisonAgent]) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let names = agents
        .iter()
        .map(|agent| agent.kind.name())
        .collect::<Vec<_>>()
        .join("-vs-");
    format!("{millis}-{names}-{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests;
