use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use clap::{Args, Subcommand, ValueEnum};
use imp_core::config::Config;
use imp_core::session::SessionManager;
use imp_core::usage::{
    dedupe_usage_records, SessionUsageRecord, UsageCostBreakdown, UsageRecordSource, UsageTokens,
};
use serde::Serialize;

use crate::BoundKind;

#[derive(Subcommand, Debug)]
pub(crate) enum UsageCommand {
    /// Show overall usage totals
    Summary(UsageReportArgs),
    /// Show usage grouped by day
    Daily(UsageReportArgs),
    /// Show usage grouped by model
    Models(UsageReportArgs),
    /// Show usage grouped by session
    Sessions(UsageReportArgs),
    /// Export usage records in a machine-friendly format
    Export(UsageExportArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UsageExportFormat {
    Json,
}

#[derive(Debug, Clone, Args)]
pub(crate) struct UsageReportArgs {
    /// Include records on or after this unix timestamp or YYYY-MM-DD date
    #[arg(long)]
    since: Option<String>,
    /// Include records before this unix timestamp or date
    #[arg(long)]
    until: Option<String>,
    /// Only include this provider
    #[arg(long)]
    provider: Option<String>,
    /// Only include this model
    #[arg(long)]
    model: Option<String>,
    /// Only include this session id or path fragment
    #[arg(long)]
    session: Option<String>,
    /// Emit JSON instead of a human table when supported
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
pub(crate) struct UsageExportArgs {
    #[command(flatten)]
    filters: UsageReportArgs,
    /// Export format
    #[arg(long, value_enum, default_value_t = UsageExportFormat::Json)]
    format: UsageExportFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageReportKind {
    Summary,
    Daily,
    Models,
    Sessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum UsageGroupKind {
    Day,
    Model,
    Session,
}

#[derive(Debug, Clone)]
struct UsageFilters {
    since: Option<u64>,
    until: Option<u64>,
    provider: Option<String>,
    model: Option<String>,
    session: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct UsageTotalsRow {
    requests: usize,
    tokens: UsageTokens,
    cost: UsageCostBreakdown,
}

#[derive(Debug, Clone, Serialize)]
struct UsageGroupRow {
    group: String,
    group_kind: UsageGroupKind,
    provider: Option<String>,
    model: Option<String>,
    session_id: Option<String>,
    session_path: Option<String>,
    day: Option<String>,
    totals: UsageTotalsRow,
}

#[derive(Debug, Clone, Serialize)]
struct UsageSessionSummary {
    session_id: Option<String>,
    session_path: Option<String>,
    messages: usize,
    first_timestamp: Option<u64>,
    last_timestamp: Option<u64>,
    first_day: Option<String>,
    last_day: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageFilterSummary {
    since: Option<u64>,
    until: Option<u64>,
    provider: Option<String>,
    model: Option<String>,
    session: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageSummaryJson {
    report: &'static str,
    generated_at: u64,
    filters: UsageFilterSummary,
    totals: UsageTotalsRow,
    sessions: usize,
    providers: usize,
    models: usize,
    canonical_records: usize,
    legacy_records: usize,
}

#[derive(Debug, Clone, Serialize)]
struct UsageGroupedJson {
    report: &'static str,
    generated_at: u64,
    filters: UsageFilterSummary,
    totals: UsageTotalsRow,
    rows: Vec<UsageGroupRow>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageExportJson {
    report: &'static str,
    generated_at: u64,
    filters: UsageFilterSummary,
    totals: UsageTotalsRow,
    records: Vec<UsageExportRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageExportRecord {
    request_id: String,
    recorded_at: u64,
    day: String,
    provider: Option<String>,
    model: Option<String>,
    session: UsageSessionSummary,
    source: UsageRecordSource,
    tokens: UsageTokens,
    cost: Option<UsageCostBreakdown>,
    assistant_message_id: Option<String>,
    turn_index: Option<u32>,
    entry_id: String,
    parent_id: Option<String>,
}

pub fn run_usage_command(command: &UsageCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        UsageCommand::Summary(args) => run_usage_report(UsageReportKind::Summary, args),
        UsageCommand::Daily(args) => run_usage_report(UsageReportKind::Daily, args),
        UsageCommand::Models(args) => run_usage_report(UsageReportKind::Models, args),
        UsageCommand::Sessions(args) => run_usage_report(UsageReportKind::Sessions, args),
        UsageCommand::Export(args) => run_usage_export(args),
    }
}

fn run_usage_report(
    kind: UsageReportKind,
    args: &UsageReportArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let filters = UsageFilters::from_args(args)?;
    let records = load_filtered_usage_records_from_dir(&Config::session_dir(), &filters)?;

    if args.json {
        match kind {
            UsageReportKind::Summary => {
                print_json_pretty(&build_usage_summary_json(&filters, &records))?
            }
            UsageReportKind::Daily => print_json_pretty(&build_usage_grouped_json(
                "daily",
                &filters,
                build_daily_rows(&records),
                &records,
            ))?,
            UsageReportKind::Models => print_json_pretty(&build_usage_grouped_json(
                "models",
                &filters,
                build_model_rows(&records),
                &records,
            ))?,
            UsageReportKind::Sessions => print_json_pretty(&build_usage_grouped_json(
                "sessions",
                &filters,
                build_session_rows(&records),
                &records,
            ))?,
        }
        return Ok(());
    }

    match kind {
        UsageReportKind::Summary => print_usage_summary_table(&filters, &records),
        UsageReportKind::Daily => {
            print_usage_grouped_table("Daily usage", &filters, &build_daily_rows(&records))
        }
        UsageReportKind::Models => {
            print_usage_grouped_table("Usage by model", &filters, &build_model_rows(&records))
        }
        UsageReportKind::Sessions => {
            print_usage_grouped_table("Usage by session", &filters, &build_session_rows(&records))
        }
    }

    Ok(())
}

fn run_usage_export(args: &UsageExportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let filters = UsageFilters::from_args(&args.filters)?;
    let records = load_filtered_usage_records_from_dir(&Config::session_dir(), &filters)?;

    match args.format {
        UsageExportFormat::Json => print_json_pretty(&build_usage_export_json(&filters, &records))?,
    }

    Ok(())
}

impl UsageFilters {
    pub(crate) fn from_args(args: &UsageReportArgs) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            since: args
                .since
                .as_deref()
                .map(|value| parse_usage_time_bound(value, BoundKind::Since))
                .transpose()?,
            until: args
                .until
                .as_deref()
                .map(|value| parse_usage_time_bound(value, BoundKind::Until))
                .transpose()?,
            provider: normalize_optional_filter(args.provider.as_deref()),
            model: normalize_optional_filter(args.model.as_deref()),
            session: normalize_optional_filter(args.session.as_deref()),
        })
    }

    fn summary(&self) -> UsageFilterSummary {
        UsageFilterSummary {
            since: self.since,
            until: self.until,
            provider: self.provider.clone(),
            model: self.model.clone(),
            session: self.session.clone(),
        }
    }

    fn has_any(&self) -> bool {
        self.since.is_some()
            || self.until.is_some()
            || self.provider.is_some()
            || self.model.is_some()
            || self.session.is_some()
    }

    fn matches(&self, record: &SessionUsageRecord) -> bool {
        if let Some(since) = self.since {
            if record.recorded_at < since {
                return false;
            }
        }

        if let Some(until) = self.until {
            if record.recorded_at >= until {
                return false;
            }
        }

        if let Some(provider) = self.provider.as_deref() {
            let Some(record_provider) = record.provider.as_deref() else {
                return false;
            };
            if !record_provider.eq_ignore_ascii_case(provider) {
                return false;
            }
        }

        if let Some(model) = self.model.as_deref() {
            let Some(record_model) = record.model.as_deref() else {
                return false;
            };
            if !record_model.eq_ignore_ascii_case(model) {
                return false;
            }
        }

        if let Some(session) = self.session.as_deref() {
            let session_lower = session.to_ascii_lowercase();
            let session_id_matches = record
                .session_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(session));
            let session_path_matches = record
                .session_path
                .as_deref()
                .map(|path| path.to_ascii_lowercase().contains(&session_lower))
                .unwrap_or(false);
            if !session_id_matches && !session_path_matches {
                return false;
            }
        }

        true
    }
}

fn normalize_optional_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn parse_usage_time_bound(
    raw: &str,
    kind: BoundKind,
) -> Result<u64, Box<dyn std::error::Error>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(io::Error::other("time bound cannot be empty").into());
    }

    if let Ok(timestamp) = trimmed.parse::<u64>() {
        return Ok(timestamp);
    }

    let (year, month, day) = parse_yyyy_mm_dd(trimmed).ok_or_else(|| {
        io::Error::other(format!(
            "invalid time bound '{trimmed}': expected unix timestamp or YYYY-MM-DD"
        ))
    })?;
    let day_start = day_start_timestamp(year, month, day)
        .ok_or_else(|| io::Error::other(format!("invalid calendar date '{trimmed}'")))?;

    Ok(match kind {
        BoundKind::Since => day_start,
        BoundKind::Until => day_start.saturating_add(86_400),
    })
}

fn parse_yyyy_mm_dd(value: &str) -> Option<(i32, u32, u32)> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((year, month, day))
}

fn day_start_timestamp(year: i32, month: u32, day: u32) -> Option<u64> {
    if !(1..=12).contains(&month) {
        return None;
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return None;
    }

    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    Some((days as u64) * 86_400)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn format_utc_day(timestamp: u64) -> String {
    let (year, month, day) = civil_from_days((timestamp / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

fn load_filtered_usage_records_from_dir(
    session_dir: &Path,
    filters: &UsageFilters,
) -> Result<Vec<SessionUsageRecord>, Box<dyn std::error::Error>> {
    let all_records = load_usage_records_from_dir(session_dir)?;
    let deduped = dedupe_usage_records(&all_records);
    Ok(deduped
        .into_iter()
        .filter(|record| filters.matches(record))
        .collect())
}

fn load_usage_records_from_dir(
    session_dir: &Path,
) -> Result<Vec<SessionUsageRecord>, Box<dyn std::error::Error>> {
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for dir_entry in fs::read_dir(session_dir)? {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            paths.push(path);
        }
    }
    paths.sort();

    let mut records = Vec::new();
    for path in paths {
        let session = SessionManager::open(&path)?;
        records.extend(session.usage_records());
    }

    Ok(records)
}

fn build_usage_summary_json(
    filters: &UsageFilters,
    records: &[SessionUsageRecord],
) -> UsageSummaryJson {
    UsageSummaryJson {
        report: "summary",
        generated_at: imp_llm::now(),
        filters: filters.summary(),
        totals: totals_from_records(records),
        sessions: count_unique_sessions(records),
        providers: count_unique_providers(records),
        models: count_unique_models(records),
        canonical_records: records
            .iter()
            .filter(|record| record.source == UsageRecordSource::Canonical)
            .count(),
        legacy_records: records
            .iter()
            .filter(|record| record.source == UsageRecordSource::LegacyAssistantMessage)
            .count(),
    }
}

fn build_usage_grouped_json(
    report: &'static str,
    filters: &UsageFilters,
    rows: Vec<UsageGroupRow>,
    records: &[SessionUsageRecord],
) -> UsageGroupedJson {
    UsageGroupedJson {
        report,
        generated_at: imp_llm::now(),
        filters: filters.summary(),
        totals: totals_from_records(records),
        rows,
    }
}

fn build_usage_export_json(
    filters: &UsageFilters,
    records: &[SessionUsageRecord],
) -> UsageExportJson {
    let session_summaries = build_session_summaries(records);
    let mut export_records: Vec<_> = records
        .iter()
        .map(|record| UsageExportRecord {
            request_id: record.request_id.clone(),
            recorded_at: record.recorded_at,
            day: format_utc_day(record.recorded_at),
            provider: record.provider.clone(),
            model: record.model.clone(),
            session: session_summaries
                .get(&session_identity_from_record(record))
                .cloned()
                .unwrap_or_else(|| session_summary_from_record(record)),
            source: record.source,
            tokens: record.usage.clone(),
            cost: record.cost.clone(),
            assistant_message_id: record.assistant_message_id.clone(),
            turn_index: record.turn_index,
            entry_id: record.entry_id.clone(),
            parent_id: record.parent_id.clone(),
        })
        .collect();
    export_records.sort_by(|a, b| {
        a.recorded_at
            .cmp(&b.recorded_at)
            .then_with(|| a.request_id.cmp(&b.request_id))
    });

    UsageExportJson {
        report: "export",
        generated_at: imp_llm::now(),
        filters: filters.summary(),
        totals: totals_from_records(records),
        records: export_records,
    }
}

fn build_daily_rows(records: &[SessionUsageRecord]) -> Vec<UsageGroupRow> {
    let mut rows: HashMap<String, UsageGroupRow> = HashMap::new();
    for record in records {
        let day = format_utc_day(record.recorded_at);
        let row = rows.entry(day.clone()).or_insert_with(|| UsageGroupRow {
            group: day.clone(),
            group_kind: UsageGroupKind::Day,
            provider: None,
            model: None,
            session_id: None,
            session_path: None,
            day: Some(day.clone()),
            totals: UsageTotalsRow::default(),
        });
        add_record_to_totals(&mut row.totals, record);
    }

    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| b.group.cmp(&a.group));
    rows
}

fn build_model_rows(records: &[SessionUsageRecord]) -> Vec<UsageGroupRow> {
    let mut rows: HashMap<(Option<String>, Option<String>), UsageGroupRow> = HashMap::new();
    for record in records {
        let key = (record.provider.clone(), record.model.clone());
        let provider = record.provider.clone();
        let model = record.model.clone();
        let label = model_display_label(provider.as_deref(), model.as_deref());
        let row = rows.entry(key).or_insert_with(|| UsageGroupRow {
            group: label,
            group_kind: UsageGroupKind::Model,
            provider,
            model,
            session_id: None,
            session_path: None,
            day: None,
            totals: UsageTotalsRow::default(),
        });
        add_record_to_totals(&mut row.totals, record);
    }

    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        b.totals
            .cost
            .total
            .partial_cmp(&a.totals.cost.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.totals.requests.cmp(&a.totals.requests))
            .then_with(|| a.group.cmp(&b.group))
    });
    rows
}

fn build_session_rows(records: &[SessionUsageRecord]) -> Vec<UsageGroupRow> {
    let mut rows: HashMap<String, UsageGroupRow> = HashMap::new();
    for record in records {
        let key = session_identity_from_record(record);
        let label =
            session_display_label(record.session_id.as_deref(), record.session_path.as_deref());
        let row = rows.entry(key).or_insert_with(|| UsageGroupRow {
            group: label,
            group_kind: UsageGroupKind::Session,
            provider: None,
            model: None,
            session_id: record.session_id.clone(),
            session_path: record.session_path.clone(),
            day: None,
            totals: UsageTotalsRow::default(),
        });
        add_record_to_totals(&mut row.totals, record);
    }

    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        b.totals
            .cost
            .total
            .partial_cmp(&a.totals.cost.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.totals.requests.cmp(&a.totals.requests))
            .then_with(|| a.group.cmp(&b.group))
    });
    rows
}

fn build_session_summaries(records: &[SessionUsageRecord]) -> HashMap<String, UsageSessionSummary> {
    let mut summaries = HashMap::new();
    for record in records {
        let key = session_identity_from_record(record);
        let summary = summaries
            .entry(key)
            .or_insert_with(|| session_summary_from_record(record));
        summary.messages += 1;
        summary.first_timestamp = Some(
            summary
                .first_timestamp
                .map(|ts| ts.min(record.recorded_at))
                .unwrap_or(record.recorded_at),
        );
        summary.last_timestamp = Some(
            summary
                .last_timestamp
                .map(|ts| ts.max(record.recorded_at))
                .unwrap_or(record.recorded_at),
        );
        summary.first_day = summary.first_timestamp.map(format_utc_day);
        summary.last_day = summary.last_timestamp.map(format_utc_day);
    }
    summaries
}

fn session_summary_from_record(record: &SessionUsageRecord) -> UsageSessionSummary {
    UsageSessionSummary {
        session_id: record.session_id.clone(),
        session_path: record.session_path.clone(),
        messages: 0,
        first_timestamp: Some(record.recorded_at),
        last_timestamp: Some(record.recorded_at),
        first_day: Some(format_utc_day(record.recorded_at)),
        last_day: Some(format_utc_day(record.recorded_at)),
    }
}

fn totals_from_records(records: &[SessionUsageRecord]) -> UsageTotalsRow {
    let mut totals = UsageTotalsRow::default();
    for record in records {
        add_record_to_totals(&mut totals, record);
    }
    totals
}

fn add_record_to_totals(totals: &mut UsageTotalsRow, record: &SessionUsageRecord) {
    totals.requests += 1;
    totals.tokens.input += record.usage.input;
    totals.tokens.output += record.usage.output;
    totals.tokens.cache_read += record.usage.cache_read;
    totals.tokens.cache_write += record.usage.cache_write;
    if let Some(cost) = &record.cost {
        totals.cost.input += cost.input;
        totals.cost.output += cost.output;
        totals.cost.cache_read += cost.cache_read;
        totals.cost.cache_write += cost.cache_write;
        totals.cost.total += cost.total;
    }
}

fn count_unique_sessions(records: &[SessionUsageRecord]) -> usize {
    records
        .iter()
        .map(session_identity_from_record)
        .collect::<HashSet<_>>()
        .len()
}

fn count_unique_providers(records: &[SessionUsageRecord]) -> usize {
    records
        .iter()
        .filter_map(|record| record.provider.clone())
        .collect::<HashSet<_>>()
        .len()
}

fn count_unique_models(records: &[SessionUsageRecord]) -> usize {
    records
        .iter()
        .filter_map(|record| match (&record.provider, &record.model) {
            (Some(provider), Some(model)) => Some(format!("{provider}:{model}")),
            (_, Some(model)) => Some(model.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>()
        .len()
}

fn session_identity_from_record(record: &SessionUsageRecord) -> String {
    session_identity(record.session_id.as_deref(), record.session_path.as_deref())
}

fn session_identity(session_id: Option<&str>, session_path: Option<&str>) -> String {
    match (session_id, session_path) {
        (Some(id), Some(path)) => format!("{id}|{path}"),
        (Some(id), None) => format!("id:{id}"),
        (None, Some(path)) => format!("path:{path}"),
        (None, None) => "unknown-session".to_string(),
    }
}

fn model_display_label(provider: Option<&str>, model: Option<&str>) -> String {
    match (provider, model) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        (None, Some(model)) => format!("(unknown provider)/{model}"),
        (Some(provider), None) => format!("{provider}/(unknown model)"),
        (None, None) => "(legacy usage with unknown model)".to_string(),
    }
}

fn session_display_label(session_id: Option<&str>, session_path: Option<&str>) -> String {
    let short_id = session_id.map(|id| truncate_middle(id, 12));
    let file_name = session_path.and_then(|path| {
        Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
    });

    match (short_id, file_name) {
        (Some(id), Some(name)) if id != name => format!("{id} ({name})"),
        (Some(id), _) => id,
        (None, Some(name)) => name,
        (None, None) => "(unknown session)".to_string(),
    }
}

fn print_usage_summary_table(filters: &UsageFilters, records: &[SessionUsageRecord]) {
    let totals = totals_from_records(records);
    println!("Usage summary");
    if filters.has_any() {
        println!("Filters: {}", format_filter_summary(filters));
    }
    println!();
    println!("{:<18} {:>14}", "Requests", format_count(totals.requests));
    println!(
        "{:<18} {:>14}",
        "Sessions",
        format_count(count_unique_sessions(records))
    );
    println!(
        "{:<18} {:>14}",
        "Providers",
        format_count(count_unique_providers(records))
    );
    println!(
        "{:<18} {:>14}",
        "Models",
        format_count(count_unique_models(records))
    );
    println!(
        "{:<18} {:>14}",
        "Canonical rows",
        format_count(
            records
                .iter()
                .filter(|record| record.source == UsageRecordSource::Canonical)
                .count()
        )
    );
    println!(
        "{:<18} {:>14}",
        "Legacy rows",
        format_count(
            records
                .iter()
                .filter(|record| record.source == UsageRecordSource::LegacyAssistantMessage)
                .count()
        )
    );
    println!();
    print_usage_totals_table(&totals);
}

fn print_usage_grouped_table(title: &str, filters: &UsageFilters, rows: &[UsageGroupRow]) {
    println!("{title}");
    if filters.has_any() {
        println!("Filters: {}", format_filter_summary(filters));
    }
    println!();

    let group_width = rows
        .iter()
        .map(|row| row.group.chars().count())
        .max()
        .unwrap_or(8)
        .clamp(8, 42);
    println!(
        "{:<group_width$} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "GROUP",
        "REQS",
        "INPUT",
        "OUTPUT",
        "CACHE RD",
        "CACHE WR",
        "COST",
        group_width = group_width,
    );
    println!("{}", "-".repeat(group_width + 74));

    for row in rows {
        println!(
            "{:<group_width$} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12}",
            truncate_middle(&row.group, group_width),
            format_count(row.totals.requests),
            format_u32(row.totals.tokens.input),
            format_u32(row.totals.tokens.output),
            format_u32(row.totals.tokens.cache_read),
            format_u32(row.totals.tokens.cache_write),
            format_currency(row.totals.cost.total),
            group_width = group_width,
        );
    }

    if rows.is_empty() {
        println!("(no usage records found)");
        return;
    }

    println!("{}", "-".repeat(group_width + 74));
    let totals = totals_from_group_rows(rows);
    println!(
        "{:<group_width$} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "TOTAL",
        format_count(totals.requests),
        format_u32(totals.tokens.input),
        format_u32(totals.tokens.output),
        format_u32(totals.tokens.cache_read),
        format_u32(totals.tokens.cache_write),
        format_currency(totals.cost.total),
        group_width = group_width,
    );
}

fn totals_from_group_rows(rows: &[UsageGroupRow]) -> UsageTotalsRow {
    let mut totals = UsageTotalsRow::default();
    for row in rows {
        totals.requests += row.totals.requests;
        totals.tokens.input += row.totals.tokens.input;
        totals.tokens.output += row.totals.tokens.output;
        totals.tokens.cache_read += row.totals.tokens.cache_read;
        totals.tokens.cache_write += row.totals.tokens.cache_write;
        totals.cost.input += row.totals.cost.input;
        totals.cost.output += row.totals.cost.output;
        totals.cost.cache_read += row.totals.cost.cache_read;
        totals.cost.cache_write += row.totals.cost.cache_write;
        totals.cost.total += row.totals.cost.total;
    }
    totals
}

fn print_usage_totals_table(totals: &UsageTotalsRow) {
    println!("{:<12} {:>12} {:>12}", "TOKENS", "COUNT", "STORED COST");
    println!("{}", "-".repeat(40));
    println!(
        "{:<12} {:>12} {:>12}",
        "Input",
        format_u32(totals.tokens.input),
        format_currency(totals.cost.input)
    );
    println!(
        "{:<12} {:>12} {:>12}",
        "Output",
        format_u32(totals.tokens.output),
        format_currency(totals.cost.output)
    );
    println!(
        "{:<12} {:>12} {:>12}",
        "Cache read",
        format_u32(totals.tokens.cache_read),
        format_currency(totals.cost.cache_read)
    );
    println!(
        "{:<12} {:>12} {:>12}",
        "Cache write",
        format_u32(totals.tokens.cache_write),
        format_currency(totals.cost.cache_write)
    );
    println!("{}", "-".repeat(40));
    println!(
        "{:<12} {:>12} {:>12}",
        "Total",
        format_u32(
            totals.tokens.input
                + totals.tokens.output
                + totals.tokens.cache_read
                + totals.tokens.cache_write,
        ),
        format_currency(totals.cost.total)
    );
}

fn format_filter_summary(filters: &UsageFilters) -> String {
    let mut parts = Vec::new();
    if let Some(since) = filters.since {
        parts.push(format!("since {}", format_utc_day(since)));
    }
    if let Some(until) = filters.until {
        parts.push(format!("until {}", format_utc_day(until.saturating_sub(1))));
    }
    if let Some(provider) = &filters.provider {
        parts.push(format!("provider={provider}"));
    }
    if let Some(model) = &filters.model {
        parts.push(format!("model={model}"));
    }
    if let Some(session) = &filters.session {
        parts.push(format!("session={session}"));
    }
    parts.join(", ")
}

fn format_count(value: usize) -> String {
    format_number(value as u64)
}

fn format_u32(value: u32) -> String {
    format_number(value as u64)
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().rev().enumerate() {
        if index != 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn format_currency(value: f64) -> String {
    if value >= 100.0 {
        format!("${value:.2}")
    } else if value >= 1.0 {
        format!("${value:.4}")
    } else {
        format!("${value:.6}")
    }
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<_> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let head = (max_chars - 1) / 2;
    let tail = max_chars - 1 - head;
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}

fn print_json_pretty<T: Serialize>(value: &T) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
#[path = "usage_report/tests.rs"]
mod tests;
