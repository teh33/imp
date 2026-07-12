use std::path::PathBuf;

use serde::Serialize;

use super::super::result::{EvalRunResult, EvalRunStatus};
use super::usage::{cost_usd, usage_breakdown};

#[derive(Debug, Serialize)]
pub(crate) struct ComparisonEvalReport {
    pub schema_version: u32,
    pub comparison_id: String,
    pub task: String,
    pub provider: String,
    pub model: String,
    pub thinking: String,
    pub repetitions: u32,
    pub pass_leaders: Vec<String>,
    pub agents: Vec<AgentAggregate>,
    pub runs: Vec<ComparisonRun>,
    pub report_path: PathBuf,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentAggregate {
    pub agent: String,
    pub passed: u32,
    pub failed: u32,
    pub pass_rate: f64,
    pub median_duration_ms: Option<u64>,
    pub total_raw_tokens: Option<u64>,
    pub total_effective_tokens: Option<u64>,
    pub total_input_tokens: Option<u64>,
    pub total_output_tokens: Option<u64>,
    pub total_cache_read_tokens: Option<u64>,
    pub total_cache_write_tokens: Option<u64>,
    pub total_cost_usd: Option<f64>,
    pub total_provider_requests: Option<u64>,
    pub total_provider_ms: Option<u64>,
    pub total_tool_ms: Option<u64>,
    pub total_context_assembly_ms: Option<u64>,
    pub total_turns: Option<u64>,
    pub total_tool_calls: Option<u64>,
    pub total_failed_tool_calls: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ComparisonRun {
    pub repetition: u32,
    pub execution_order: Vec<String>,
    pub agents: Vec<AgentRun>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentRun {
    pub agent: String,
    #[serde(flatten)]
    pub summary: RunSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct RunSummary {
    pub run_id: String,
    pub status: EvalRunStatus,
    pub duration_ms: Option<u64>,
    pub raw_tokens: Option<u64>,
    pub effective_tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub provider_requests: Option<u64>,
    pub provider_ms: Option<u64>,
    pub tool_ms: Option<u64>,
    pub context_assembly_ms: Option<u64>,
    pub turns: Option<u64>,
    pub tool_calls: Option<u64>,
    pub failed_tool_calls: Option<u64>,
    pub files_changed: u32,
    pub artifact_dir: PathBuf,
    pub error: Option<String>,
}

pub(super) fn summarize(result: EvalRunResult, artifact_dir: PathBuf) -> RunSummary {
    let usage = usage_breakdown(&result);
    let cost = cost_usd(&result);
    let metric = |name: &str| result.agent.outcome.as_ref()?["metrics"][name].as_u64();
    RunSummary {
        run_id: result.run_id,
        status: result.status,
        duration_ms: result.agent.duration_ms,
        raw_tokens: usage.as_ref().map(|value| value.raw),
        effective_tokens: usage.as_ref().map(|value| value.effective),
        input_tokens: usage.as_ref().map(|value| value.input),
        output_tokens: usage.as_ref().map(|value| value.output),
        cache_read_tokens: usage.as_ref().map(|value| value.cache_read),
        cache_write_tokens: usage.as_ref().map(|value| value.cache_write),
        cost_usd: cost,
        provider_requests: metric("provider_requests"),
        provider_ms: metric("provider_total_ms"),
        tool_ms: metric("tool_total_ms"),
        context_assembly_ms: metric("context_assembly_total_ms"),
        turns: metric("turns"),
        tool_calls: metric("tool_calls"),
        failed_tool_calls: metric("failed_tool_calls"),
        files_changed: result.diff.files_changed,
        artifact_dir,
        error: result.error,
    }
}

pub(super) fn aggregate(agent: &str, runs: &[&RunSummary]) -> AgentAggregate {
    let passed = runs
        .iter()
        .filter(|run| run.status == EvalRunStatus::Passed)
        .count() as u32;
    let mut durations = runs
        .iter()
        .filter_map(|run| run.duration_ms)
        .collect::<Vec<_>>();
    durations.sort_unstable();
    AgentAggregate {
        agent: agent.to_string(),
        passed,
        failed: runs.len() as u32 - passed,
        pass_rate: passed as f64 / runs.len() as f64,
        median_duration_ms: durations.get(durations.len() / 2).copied(),
        total_raw_tokens: sum_options(runs.iter().map(|run| run.raw_tokens)),
        total_effective_tokens: sum_options(runs.iter().map(|run| run.effective_tokens)),
        total_input_tokens: sum_options(runs.iter().map(|run| run.input_tokens)),
        total_output_tokens: sum_options(runs.iter().map(|run| run.output_tokens)),
        total_cache_read_tokens: sum_options(runs.iter().map(|run| run.cache_read_tokens)),
        total_cache_write_tokens: sum_options(runs.iter().map(|run| run.cache_write_tokens)),
        total_cost_usd: sum_f64_options(runs.iter().map(|run| run.cost_usd)),
        total_provider_requests: sum_options(runs.iter().map(|run| run.provider_requests)),
        total_provider_ms: sum_options(runs.iter().map(|run| run.provider_ms)),
        total_tool_ms: sum_options(runs.iter().map(|run| run.tool_ms)),
        total_context_assembly_ms: sum_options(runs.iter().map(|run| run.context_assembly_ms)),
        total_turns: sum_options(runs.iter().map(|run| run.turns)),
        total_tool_calls: sum_options(runs.iter().map(|run| run.tool_calls)),
        total_failed_tool_calls: sum_options(runs.iter().map(|run| run.failed_tool_calls)),
    }
}

pub(super) fn pass_leaders(agents: &[AgentAggregate]) -> Vec<String> {
    let most_passes = agents.iter().map(|agent| agent.passed).max().unwrap_or(0);
    agents
        .iter()
        .filter(|agent| agent.passed == most_passes)
        .map(|agent| agent.agent.clone())
        .collect()
}

fn sum_options(mut values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values.try_fold(0, |total, value| Some(total + value?))
}

fn sum_f64_options(mut values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    values.try_fold(0.0, |total, value| Some(total + value?))
}
