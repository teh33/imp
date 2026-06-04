use super::*;
use crate::session::SessionEntry;
use imp_llm::{AssistantMessage, ContentBlock, StopReason};

fn assistant_message(timestamp: u64, usage: Option<Usage>) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: "done".into(),
        }],
        usage,
        stop_reason: StopReason::EndTurn,
        timestamp,
    })
}

fn legacy_assistant_entry(id: &str, timestamp: u64, usage: Usage) -> SessionEntry {
    SessionEntry::Message {
        id: id.to_string(),
        parent_id: None,
        message: assistant_message(timestamp, Some(usage)),
    }
}

fn canonical_entry(
    entry_id: &str,
    request_id: &str,
    assistant_message_id: Option<&str>,
    session_id: Option<&str>,
    usage: Usage,
    cost: Cost,
) -> SessionEntry {
    usage_record_entry(
        entry_id,
        UsageRecordV1::new(
            request_id,
            123,
            "anthropic",
            "claude-3-7-sonnet",
            usage,
            cost,
        )
        .with_session_context(
            session_id.map(str::to_string),
            Some("/tmp/session.jsonl".into()),
            assistant_message_id.map(str::to_string),
            Some(2),
        ),
    )
    .unwrap()
}

#[test]
fn canonical_usage_record_round_trips_through_custom_entry() {
    let entry = canonical_entry(
        "entry-1",
        "req-1",
        Some("assistant-1"),
        Some("session-1"),
        Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 5,
            cache_write_tokens: 2,
        },
        Cost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write: 0.2,
            total: 3.3,
        },
    );

    let records = usage_records_from_entries(&[entry]);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.request_id, "req-1");
    assert_eq!(record.provider.as_deref(), Some("anthropic"));
    assert_eq!(record.model.as_deref(), Some("claude-3-7-sonnet"));
    assert_eq!(record.assistant_message_id.as_deref(), Some("assistant-1"));
    assert_eq!(record.turn_index, Some(2));
    assert_eq!(record.source, UsageRecordSource::Canonical);
    assert_eq!(record.usage.input, 100);
    assert_eq!(record.cost.as_ref().unwrap().total, 3.3);
}

#[test]
fn usage_reader_falls_back_to_legacy_assistant_usage() {
    let entries = vec![legacy_assistant_entry(
        "assistant-legacy",
        456,
        Usage {
            input_tokens: 50,
            output_tokens: 10,
            cache_read_tokens: 3,
            cache_write_tokens: 0,
        },
    )];

    let records = usage_records_from_entries(&entries);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.request_id, "legacy-assistant:assistant-legacy");
    assert_eq!(record.recorded_at, 456);
    assert_eq!(record.source, UsageRecordSource::LegacyAssistantMessage);
    assert_eq!(record.provider, None);
    assert_eq!(record.model, None);
    assert_eq!(record.cost, None);
    assert_eq!(record.turn_index, Some(0));
}

#[test]
fn canonical_record_suppresses_legacy_fallback_for_same_assistant_message() {
    let usage = Usage {
        input_tokens: 80,
        output_tokens: 12,
        cache_read_tokens: 4,
        cache_write_tokens: 1,
    };
    let entries = vec![
        legacy_assistant_entry("assistant-1", 100, usage.clone()),
        canonical_entry(
            "usage-1",
            "req-1",
            Some("assistant-1"),
            Some("session-1"),
            usage,
            Cost {
                input: 0.8,
                output: 0.12,
                cache_read: 0.04,
                cache_write: 0.01,
                total: 0.97,
            },
        ),
    ];

    let records = usage_records_from_entries(&entries);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, UsageRecordSource::Canonical);
    assert_eq!(records[0].request_id, "req-1");
}

#[test]
fn aggregate_usage_dedupes_forked_history_by_request_id() {
    let usage = Usage {
        input_tokens: 100,
        output_tokens: 25,
        cache_read_tokens: 10,
        cache_write_tokens: 5,
    };
    let cost = Cost {
        input: 1.0,
        output: 2.0,
        cache_read: 0.3,
        cache_write: 0.4,
        total: 3.7,
    };
    let original = usage_records_from_entries(&[canonical_entry(
        "usage-original",
        "req-shared",
        Some("assistant-1"),
        Some("session-a"),
        usage.clone(),
        cost.clone(),
    )]);
    let forked = usage_records_from_entries(&[canonical_entry(
        "usage-fork",
        "req-shared",
        Some("assistant-1"),
        Some("session-b"),
        usage,
        cost,
    )]);

    let mut all = Vec::new();
    all.extend(original);
    all.extend(forked);

    let raw = aggregate_usage(&all);
    assert_eq!(raw.records, 2);
    assert_eq!(raw.usage.input_tokens, 200);

    let deduped = aggregate_usage_deduped(&all);
    assert_eq!(deduped.records, 1);
    assert_eq!(deduped.usage.input_tokens, 100);
    assert_eq!(deduped.usage.output_tokens, 25);
    assert!((deduped.cost.total - 3.7).abs() < f64::EPSILON);
}

#[test]
fn dedupe_usage_records_keeps_earliest_duplicate_row() {
    let usage = Usage {
        input_tokens: 100,
        output_tokens: 25,
        cache_read_tokens: 10,
        cache_write_tokens: 5,
    };
    let cost = Cost {
        input: 1.0,
        output: 2.0,
        cache_read: 0.3,
        cache_write: 0.4,
        total: 3.7,
    };

    let records = vec![
        SessionUsageRecord {
            entry_id: "late".into(),
            parent_id: None,
            request_id: "req-shared".into(),
            recorded_at: 200,
            provider: Some("anthropic".into()),
            model: Some("claude-3-7-sonnet".into()),
            session_id: Some("session-b".into()),
            session_path: Some("/tmp/b.jsonl".into()),
            assistant_message_id: Some("assistant-1".into()),
            turn_index: Some(0),
            usage: UsageTokens::from(usage.clone()),
            cost: Some(UsageCostBreakdown::from(cost.clone())),
            source: UsageRecordSource::Canonical,
        },
        SessionUsageRecord {
            entry_id: "early".into(),
            parent_id: None,
            request_id: "req-shared".into(),
            recorded_at: 100,
            provider: Some("anthropic".into()),
            model: Some("claude-3-7-sonnet".into()),
            session_id: Some("session-a".into()),
            session_path: Some("/tmp/a.jsonl".into()),
            assistant_message_id: Some("assistant-1".into()),
            turn_index: Some(0),
            usage: UsageTokens::from(usage),
            cost: Some(UsageCostBreakdown::from(cost)),
            source: UsageRecordSource::Canonical,
        },
    ];

    let deduped = dedupe_usage_records(&records);
    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].entry_id, "early");
    assert_eq!(deduped[0].session_id.as_deref(), Some("session-a"));
}

#[test]
fn aggregate_usage_keeps_distinct_legacy_records() {
    let records = usage_records_from_entries(&[
        legacy_assistant_entry(
            "assistant-1",
            100,
            Usage {
                input_tokens: 10,
                output_tokens: 2,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
        ),
        legacy_assistant_entry(
            "assistant-2",
            200,
            Usage {
                input_tokens: 20,
                output_tokens: 4,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
        ),
    ]);

    let totals = aggregate_usage_deduped(&records);
    assert_eq!(totals.records, 2);
    assert_eq!(totals.usage.input_tokens, 30);
    assert_eq!(totals.usage.output_tokens, 6);
}
