use super::super::result::EvalRunResult;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UsageBreakdown {
    pub(super) raw: u64,
    pub(super) effective: u64,
    pub(super) input: u64,
    pub(super) output: u64,
    pub(super) cache_read: u64,
    pub(super) cache_write: u64,
}

pub(super) fn usage_breakdown(result: &EvalRunResult) -> Option<UsageBreakdown> {
    let usage = result.agent.outcome.as_ref()?.get("usage")?;
    let reported_input = usage["input_tokens"].as_u64()?;
    let output = usage["output_tokens"].as_u64()?;
    let cache_read = usage["cache_read_tokens"].as_u64().unwrap_or(0);
    let cache_write = usage["cache_write_tokens"].as_u64().unwrap_or(0);
    let input_includes_cache = usage["input_includes_cache"]
        .as_bool()
        .unwrap_or_else(|| result.agent.kind.as_deref() != Some("pi"));
    let input = if input_includes_cache {
        reported_input.saturating_sub(cache_read + cache_write)
    } else {
        reported_input
    };
    let raw = input + cache_read + cache_write + output;
    let effective = input + cache_write + output;
    Some(UsageBreakdown {
        raw,
        effective,
        input,
        output,
        cache_read,
        cache_write,
    })
}

pub(super) fn cost_usd(result: &EvalRunResult) -> Option<f64> {
    let usage = result.agent.outcome.as_ref()?.get("usage")?;
    usage["cost_usd"]
        .as_f64()
        .or_else(|| usage["cost"]["total"].as_f64())
        .or_else(|| result.agent.outcome.as_ref()?["cost"]["total"].as_f64())
        .or_else(|| result.agent.outcome.as_ref()?["metrics"]["cost_usd"].as_f64())
}
