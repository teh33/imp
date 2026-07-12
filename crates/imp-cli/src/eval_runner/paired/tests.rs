use super::usage::UsageBreakdown;
use super::*;
use crate::eval_runner::result::EvalRunStatus;
use crate::eval_runner::EvalRunResult;

fn summary(status: EvalRunStatus, duration: u64) -> report::RunSummary {
    report::RunSummary {
        run_id: "1-task-id".into(),
        status,
        duration_ms: Some(duration),
        raw_tokens: Some(10),
        effective_tokens: Some(8),
        input_tokens: Some(8),
        output_tokens: Some(2),
        cache_read_tokens: Some(2),
        cache_write_tokens: Some(0),
        cost_usd: Some(0.1),
        provider_requests: Some(2),
        provider_ms: Some(duration - 1),
        tool_ms: Some(1),
        context_assembly_ms: Some(0),
        turns: Some(2),
        tool_calls: Some(1),
        failed_tool_calls: Some(0),
        files_changed: 1,
        artifact_dir: "artifact".into(),
        error: None,
    }
}

fn result(kind: &str, input_includes_cache: Option<bool>) -> EvalRunResult {
    let spec = EvalTaskSpec {
        id: "task".into(),
        repo: "repo".into(),
        commit: "a".repeat(40),
        prompt: "prompt".into(),
        verifier: "true".into(),
        fixture: None,
        expectations: Default::default(),
        setup: None,
        max_turns: None,
        timeout_seconds: None,
        verifier_timeout_seconds: None,
    };
    let mut result = EvalRunResult::initial("run".into(), &spec, "checkout".into());
    result.agent.kind = Some(kind.into());
    result.agent.outcome = Some(serde_json::json!({"usage": {
        "input_tokens": 120, "output_tokens": 10,
        "cache_read_tokens": 60, "cache_write_tokens": 20,
        "input_includes_cache": input_includes_cache,
    }}));
    result
}

#[test]
fn aggregate_counts_passes_and_median() {
    let runs = [
        summary(EvalRunStatus::Passed, 30),
        summary(EvalRunStatus::Failed, 10),
        summary(EvalRunStatus::Passed, 20),
    ];
    let references = runs.iter().collect::<Vec<_>>();
    let aggregate = aggregate("imp", &references);
    assert_eq!(aggregate.passed, 2);
    assert_eq!(aggregate.failed, 1);
    assert_eq!(aggregate.median_duration_ms, Some(20));
    assert_eq!(aggregate.total_raw_tokens, Some(30));
    assert_eq!(aggregate.total_effective_tokens, Some(24));
}

#[test]
fn explicit_usage_schema_controls_cache_normalization() {
    assert_eq!(
        usage::usage_breakdown(&result("codex", Some(true))),
        Some(UsageBreakdown {
            raw: 130,
            effective: 70,
            input: 40,
            output: 10,
            cache_read: 60,
            cache_write: 20,
        })
    );
    assert_eq!(
        usage::usage_breakdown(&result("opencode", Some(false))),
        Some(UsageBreakdown {
            raw: 210,
            effective: 150,
            input: 120,
            output: 10,
            cache_read: 60,
            cache_write: 20,
        })
    );
}

#[test]
fn old_pi_artifacts_still_treat_input_as_uncached() {
    assert_eq!(
        usage::usage_breakdown(&result("pi", None)).map(|usage| usage.input),
        Some(120)
    );
}

#[test]
fn task_offset_distributes_initial_order() {
    let mut offsets =
        ["no-op", "one-file-fix", "preserve-dirty-files"].map(|task| task_offset(task, 4));
    offsets.sort_unstable();
    assert!(offsets.windows(2).all(|pair| pair[0] != pair[1]));
}

#[test]
fn execution_order_rotates_across_repetitions() {
    let agents = [
        agent(EvalAgentKind::Imp),
        agent(EvalAgentKind::Pi),
        agent(EvalAgentKind::Codex),
    ];
    let order = rotated_agents(&agents, 1);
    assert_eq!(
        order.iter().map(|agent| agent.kind).collect::<Vec<_>>(),
        [EvalAgentKind::Pi, EvalAgentKind::Codex, EvalAgentKind::Imp]
    );
}

#[test]
fn pass_leaders_can_report_ties() {
    let passed = summary(EvalRunStatus::Passed, 1);
    let failed = summary(EvalRunStatus::Failed, 1);
    let agents = [
        aggregate("imp", &[&passed]),
        aggregate("pi", &[&passed]),
        aggregate("codex", &[&failed]),
    ];
    assert_eq!(pass_leaders(&agents), ["imp", "pi"]);
}

fn agent(kind: EvalAgentKind) -> ComparisonAgent {
    ComparisonAgent {
        kind,
        binary: kind.name().into(),
    }
}
