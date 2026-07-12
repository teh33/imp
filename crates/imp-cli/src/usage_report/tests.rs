use super::UsageReportArgs;
use super::*;
use imp_core::usage::{UsageCostBreakdown, UsageTokens};

#[allow(clippy::too_many_arguments)]
fn canonical_record(
    request_id: &str,
    recorded_at: u64,
    provider: &str,
    model: &str,
    session_id: &str,
    session_path: &str,
    input: u32,
    output: u32,
    cache_read: u32,
    cache_write: u32,
    total_cost: f64,
) -> SessionUsageRecord {
    SessionUsageRecord {
        entry_id: format!("entry-{request_id}"),
        parent_id: None,
        request_id: request_id.to_string(),
        recorded_at,
        provider: Some(provider.to_string()),
        model: Some(model.to_string()),
        session_id: Some(session_id.to_string()),
        session_path: Some(session_path.to_string()),
        assistant_message_id: Some(format!("assistant-{request_id}")),
        turn_index: Some(0),
        usage: UsageTokens {
            input,
            output,
            cache_read,
            cache_write,
        },
        cost: Some(UsageCostBreakdown {
            input: total_cost / 4.0,
            output: total_cost / 4.0,
            cache_read: total_cost / 4.0,
            cache_write: total_cost / 4.0,
            total: total_cost,
        }),
        source: UsageRecordSource::Canonical,
    }
}

#[test]
fn parse_usage_time_bound_supports_dates_and_timestamps() {
    assert_eq!(
        parse_usage_time_bound("1970-01-02", BoundKind::Since).unwrap(),
        86_400
    );
    assert_eq!(
        parse_usage_time_bound("123", BoundKind::Since).unwrap(),
        123
    );
    assert_eq!(
        parse_usage_time_bound("1970-01-02", BoundKind::Until).unwrap(),
        172_800
    );
}

#[test]
fn usage_filters_apply_provider_model_session_and_bounds() {
    let filters = UsageFilters::from_args(&UsageReportArgs {
        since: Some("1970-01-02".into()),
        until: Some("1970-01-03".into()),
        provider: Some("anthropic".into()),
        model: Some("claude".into()),
        session: Some("session-a".into()),
        json: false,
    })
    .unwrap();

    let matching = SessionUsageRecord {
        entry_id: "e1".into(),
        parent_id: None,
        request_id: "r1".into(),
        recorded_at: 100_000,
        provider: Some("anthropic".into()),
        model: Some("claude".into()),
        session_id: Some("session-a".into()),
        session_path: Some("/tmp/session-a.jsonl".into()),
        assistant_message_id: None,
        turn_index: None,
        usage: UsageTokens::default(),
        cost: None,
        source: UsageRecordSource::Canonical,
    };
    assert!(filters.matches(&matching));

    let wrong_provider = SessionUsageRecord {
        provider: Some("openai".into()),
        ..matching.clone()
    };
    assert!(!filters.matches(&wrong_provider));
}

#[test]
fn grouped_rows_sum_tokens_and_costs() {
    let records = vec![
        canonical_record(
            "r1",
            86_400,
            "anthropic",
            "claude",
            "session-a",
            "/tmp/a.jsonl",
            100,
            20,
            5,
            2,
            1.0,
        ),
        canonical_record(
            "r2",
            86_400,
            "anthropic",
            "claude",
            "session-a",
            "/tmp/a.jsonl",
            200,
            30,
            0,
            0,
            2.0,
        ),
    ];

    let daily = build_daily_rows(&records);
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].totals.requests, 2);
    assert_eq!(daily[0].totals.tokens.input, 300);
    assert!((daily[0].totals.cost.total - 3.0).abs() < f64::EPSILON);

    let models = build_model_rows(&records);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].group, "anthropic/claude");

    let sessions = build_session_rows(&records);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].totals.tokens.output, 50);
}

#[test]
fn export_json_contains_deduped_totals() {
    let records = vec![
        canonical_record(
            "r1",
            86_400,
            "anthropic",
            "claude",
            "session-a",
            "/tmp/a.jsonl",
            100,
            20,
            5,
            2,
            1.0,
        ),
        canonical_record(
            "r2",
            172_800,
            "openai",
            "gpt",
            "session-b",
            "/tmp/b.jsonl",
            50,
            10,
            0,
            0,
            0.5,
        ),
    ];
    let filters = UsageFilters::from_args(&UsageReportArgs {
        since: None,
        until: None,
        provider: None,
        model: None,
        session: None,
        json: true,
    })
    .unwrap();
    let export = build_usage_export_json(&filters, &records);
    assert_eq!(export.records.len(), 2);
    assert_eq!(export.totals.requests, 2);
    assert_eq!(export.totals.tokens.input, 150);
    assert!((export.totals.cost.total - 1.5).abs() < f64::EPSILON);
}
