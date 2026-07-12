use imp_llm::{Message, RequestOptions};

use crate::config::AutoCompactionMode;

const STREAM_RECOVERY_FOLLOW_UP: &str = "The provider stream failed before completing the previous assistant message. Continue from the last completed conversation state. Do not repeat already completed tool side effects; if you need to retry, first inspect current state and proceed safely.";
pub(super) const MAX_STREAM_RECOVERY_ATTEMPTS: u32 = 2;
pub(super) const MAX_CONTEXT_RECOVERY_ATTEMPTS: u32 = 1;

pub(super) fn recoverable_stream_failure_message(error: &str) -> Option<String> {
    if error.contains("Provider stream failed after partial output")
        || error.contains("Provider stream failed before output")
        || error.contains("missing terminal completion event")
    {
        Some(format!(
            "{STREAM_RECOVERY_FOLLOW_UP}\n\nProvider error: {error}"
        ))
    } else {
        None
    }
}

pub(super) fn effective_display_window(
    estimate: &crate::context::RequestContextEstimate,
    observed_input_limit: Option<u32>,
) -> u32 {
    observed_input_limit
        .map(|limit| estimate.display_window.min(limit.max(1)))
        .unwrap_or(estimate.display_window)
}

pub(super) fn format_context_estimate_details(
    estimate: &crate::context::RequestContextEstimate,
    observed_input_limit: Option<u32>,
) -> String {
    let effective = observed_input_limit.unwrap_or(estimate.input_limit);
    let display = effective_display_window(estimate, observed_input_limit);
    format!(
        "estimate: input {} / effective limit {} (model limit {}, display {}, system {}, tools {}, messages {}, planned output {}, observed ceiling {})",
        estimate.input_tokens,
        effective,
        estimate.input_limit,
        display,
        estimate.system_tokens,
        estimate.tool_definition_tokens,
        estimate.message_tokens,
        estimate.output_tokens,
        observed_input_limit
            .map(|limit| limit.to_string())
            .unwrap_or_else(|| "none".to_string())
    )
}

pub(super) fn recoverable_context_failure_message(error: &str) -> bool {
    crate::error_display::format_error_for_display(error).starts_with("Context full:")
}

pub(super) fn recoverable_context_failure(error: &imp_llm::Error) -> bool {
    matches!(error, imp_llm::Error::ContextTooLong { .. })
        || matches!(error, imp_llm::Error::Provider(message) if recoverable_context_failure_message(message))
}

pub(super) fn auto_compaction_should_run(
    mode: AutoCompactionMode,
    usage: &crate::context::ContextUsage,
    trigger_ratio: f64,
) -> bool {
    if usage.limit == 0 {
        return false;
    }
    let trigger_ratio = if trigger_ratio.is_finite() {
        trigger_ratio.clamp(0.0, 1.0)
    } else {
        0.90
    };
    match mode {
        AutoCompactionMode::Disabled => false,
        AutoCompactionMode::NearThreshold => usage.ratio >= trigger_ratio,
        AutoCompactionMode::Aggressive => usage.ratio >= trigger_ratio.min(0.75),
    }
}

pub(super) fn auto_compaction_tail_tokens(
    usage: &crate::context::ContextUsage,
    target_ratio: f64,
) -> u32 {
    if usage.limit == 0 {
        return crate::compaction::AUTO_COMPACTION_RECENT_TAIL_TOKENS;
    }
    let target_ratio = if target_ratio.is_finite() {
        target_ratio.clamp(0.05, 0.95)
    } else {
        0.70
    };
    let target = (usage.limit as f64 * target_ratio).floor() as u32;
    target
        .max(16_000)
        .min(crate::compaction::AUTO_COMPACTION_RECENT_TAIL_TOKENS)
}

fn observed_input_limit_after_overflow(estimate: &crate::context::RequestContextEstimate) -> u32 {
    // If the provider rejects a request below our configured model limit, treat
    // that provider response as authoritative for this run and leave headroom
    // below the failed local estimate. This prevents imp from repeatedly
    // waiting until the same too-high local token count before trimming again.
    ((estimate.input_tokens as f64) * 0.80).floor().max(1.0) as u32
}

pub(super) fn update_observed_input_limit(
    observed_input_limit: &mut Option<u32>,
    estimate: &crate::context::RequestContextEstimate,
) -> u32 {
    let observed = observed_input_limit_after_overflow(estimate);
    let effective = observed_input_limit
        .map(|existing| existing.min(observed))
        .unwrap_or(observed);
    *observed_input_limit = Some(effective);
    effective
}

pub(super) fn apply_provider_context_baseline(
    estimate: &mut crate::context::RequestContextEstimate,
    provider_context_baseline_tokens: Option<u32>,
) {
    if let Some(baseline) = provider_context_baseline_tokens {
        estimate.input_tokens = estimate.input_tokens.max(baseline);
    }
}

pub(super) fn sanitized_request_estimate(
    messages: &[Message],
    model: &imp_llm::Model,
    options: &RequestOptions,
    observed_input_limit: Option<u32>,
    provider_context_baseline_tokens: Option<u32>,
) -> (Vec<Message>, crate::context::RequestContextEstimate) {
    let mut context_messages = messages.to_vec();
    crate::session::sanitize_messages(&mut context_messages);
    let mut estimate = crate::context::estimate_request_context(&context_messages, model, options);
    apply_provider_context_baseline(&mut estimate, provider_context_baseline_tokens);
    if let Some(limit) = observed_input_limit {
        let effective_limit = limit.max(1);
        estimate.input_limit = estimate.input_limit.min(effective_limit);
        estimate.display_window = estimate.display_window.min(effective_limit);
    }
    (context_messages, estimate)
}

pub(super) fn mask_all_observations_for_recovery(
    messages: &mut [Message],
    model: &imp_llm::Model,
    options: &RequestOptions,
    observed_input_limit: Option<u32>,
    provider_context_baseline_tokens: Option<u32>,
) -> Option<(u32, u32)> {
    let before = sanitized_request_estimate(
        messages,
        model,
        options,
        observed_input_limit,
        provider_context_baseline_tokens,
    )
    .1
    .input_tokens;
    crate::context::mask_observations(messages, 0);
    let after = sanitized_request_estimate(messages, model, options, observed_input_limit, None)
        .1
        .input_tokens;
    (after < before).then_some((before, after))
}
