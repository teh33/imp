use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use imp_core::config::Config;
use imp_core::session::{SessionEntry, SessionManager};
use imp_core::usage::{dedupe_usage_records, SessionUsageRecord};
use imp_llm::Message;
use serde::Serialize;

use crate::reporting::BoundKind;

#[derive(Subcommand, Debug)]
pub(crate) enum StatsCommand {
    /// Show overall local imp stats
    Summary(StatsReportArgs),
    /// Show token and cost stats
    Tokens(StatsReportArgs),
    /// Show stats grouped by tool
    Tools(StatsReportArgs),
    /// Show file/code-change stats
    Files(StatsReportArgs),
    /// Show stats grouped by day
    Daily(StatsReportArgs),
    /// Show stats grouped by week
    Weekly(StatsReportArgs),
    /// Show stats grouped by project/session directory hint
    Projects(StatsReportArgs),
    /// Show stats grouped by session
    Sessions(StatsReportArgs),
    /// Show a fun local imp wrapped summary
    Wrapped(StatsReportArgs),
    /// Export local imp stats records in a machine-friendly format
    Export(StatsExportArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
enum StatsExportFormat {
    Json,
}

#[derive(Debug, Clone, Args)]
pub(crate) struct StatsReportArgs {
    /// Include records on or after this unix timestamp or YYYY-MM-DD date
    #[arg(long)]
    since: Option<String>,
    /// Include records before this unix timestamp or date
    #[arg(long)]
    until: Option<String>,
    /// Only include this session id or path fragment
    #[arg(long)]
    session: Option<String>,
    /// Only include this tool name
    #[arg(long)]
    tool: Option<String>,
    /// Emit JSON instead of a human table when supported
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
pub(crate) struct StatsExportArgs {
    #[command(flatten)]
    filters: StatsReportArgs,
    /// Export format
    #[arg(long, value_enum, default_value_t = StatsExportFormat::Json)]
    format: StatsExportFormat,
}

#[derive(Debug, Clone)]
struct StatsFilters {
    since: Option<u64>,
    until: Option<u64>,
    session: Option<String>,
    tool: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StatsFilterSummary {
    since: Option<u64>,
    until: Option<u64>,
    session: Option<String>,
    tool: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StatsSummaryJson {
    report: &'static str,
    generated_at: u64,
    filters: StatsFilterSummary,
    sessions: usize,
    tool_calls: usize,
    tool_errors: usize,
    unique_tools: usize,
    files_created: usize,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
    token_requests: usize,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    total_tokens: u64,
    total_cost: f64,
}

#[derive(Debug, Clone, Serialize)]
struct StatsToolRow {
    tool: String,
    calls: usize,
    errors: usize,
    files_created: usize,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
}

pub fn run_stats_command(command: &StatsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        StatsCommand::Summary(args) => run_stats_summary(args),
        StatsCommand::Tokens(args) => run_stats_tokens(args),
        StatsCommand::Tools(args) => run_stats_tools(args),
        StatsCommand::Files(args) => run_stats_files(args),
        StatsCommand::Daily(args) => run_stats_periods(args, PeriodKind::Day),
        StatsCommand::Weekly(args) => run_stats_periods(args, PeriodKind::Week),
        StatsCommand::Projects(args) => run_stats_projects(args),
        StatsCommand::Sessions(args) => run_stats_sessions(args),
        StatsCommand::Wrapped(args) => run_stats_wrapped(args),
        StatsCommand::Export(args) => run_stats_export(args),
    }
}

fn run_stats_summary(args: &StatsReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let summary = build_stats_summary_json(
        &dataset.filters,
        &dataset.tool_records,
        &dataset.usage_records,
    );
    if args.json {
        print_json_pretty(&summary)?;
    } else {
        print_stats_summary_table(&summary);
    }
    Ok(())
}

fn run_stats_tokens(args: &StatsReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let summary = build_stats_summary_json(
        &dataset.filters,
        &dataset.tool_records,
        &dataset.usage_records,
    );
    if args.json {
        print_json_pretty(&summary)?;
    } else {
        println!("Imp token stats");
        println!("  requests           {}", summary.token_requests);
        println!("  input tokens       {}", summary.input_tokens);
        println!("  output tokens      {}", summary.output_tokens);
        println!("  cache read tokens  {}", summary.cache_read_tokens);
        println!("  cache write tokens {}", summary.cache_write_tokens);
        println!("  total tokens       {}", summary.total_tokens);
        println!("  total cost         ${:.4}", summary.total_cost);
    }
    Ok(())
}

fn run_stats_tools(args: &StatsReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let rows = build_tool_rows(&dataset.tool_records);
    if args.json {
        print_json_pretty(&rows)?;
    } else {
        print_tool_rows(&rows);
    }
    Ok(())
}

fn run_stats_files(args: &StatsReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let summary = build_stats_summary_json(
        &dataset.filters,
        &dataset.tool_records,
        &dataset.usage_records,
    );
    if args.json {
        print_json_pretty(&summary)?;
    } else {
        println!("Imp file stats");
        println!("  files created  {}", summary.files_created);
        println!("  lines read     {}", summary.lines_read);
        println!("  lines added    {}", summary.lines_added);
        println!("  lines removed  {}", summary.lines_removed);
        println!(
            "  net lines      {}",
            summary.lines_added as i64 - summary.lines_removed as i64
        );
    }
    Ok(())
}

fn run_stats_periods(
    args: &StatsReportArgs,
    kind: PeriodKind,
) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let rows = build_period_rows(&dataset, kind);
    if args.json {
        print_json_pretty(&rows)?;
    } else {
        print_period_rows(kind, &rows);
    }
    Ok(())
}

fn run_stats_projects(args: &StatsReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let rows = build_project_rows(&dataset.tool_records);
    if args.json {
        print_json_pretty(&rows)?;
    } else {
        print_named_rows("project", &rows);
    }
    Ok(())
}

fn run_stats_sessions(args: &StatsReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let rows = build_session_rows(&dataset);
    if args.json {
        print_json_pretty(&rows)?;
    } else {
        print_named_rows("session", &rows);
    }
    Ok(())
}

fn run_stats_wrapped(args: &StatsReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(args)?;
    let wrapped = build_wrapped(&dataset);
    if args.json {
        print_json_pretty(&wrapped)?;
    } else {
        print_wrapped(&wrapped);
    }
    Ok(())
}

fn run_stats_export(args: &StatsExportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = load_stats_dataset(&args.filters)?;
    match args.format {
        StatsExportFormat::Json => print_json_pretty(&build_stats_export_json(&dataset))?,
    }
    Ok(())
}

impl StatsFilters {
    pub(crate) fn from_args(args: &StatsReportArgs) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            since: args
                .since
                .as_deref()
                .map(|v| crate::usage_report::parse_usage_time_bound(v, BoundKind::Since))
                .transpose()?,
            until: args
                .until
                .as_deref()
                .map(|v| crate::usage_report::parse_usage_time_bound(v, BoundKind::Until))
                .transpose()?,
            session: normalize_optional_filter(args.session.as_deref()),
            tool: normalize_optional_filter(args.tool.as_deref()),
        })
    }

    fn summary(&self) -> StatsFilterSummary {
        StatsFilterSummary {
            since: self.since,
            until: self.until,
            session: self.session.clone(),
            tool: self.tool.clone(),
        }
    }

    fn matches_tool(&self, record: &ToolStatsRecord) -> bool {
        self.matches_common(record.timestamp, &record.session_id, &record.session_path)
            && self
                .tool
                .as_deref()
                .is_none_or(|tool| record.tool_name.eq_ignore_ascii_case(tool))
    }

    fn matches_usage(&self, record: &SessionUsageRecord) -> bool {
        let session_id = record.session_id.as_deref().unwrap_or_default();
        let session_path = record.session_path.as_deref().unwrap_or_default();
        self.matches_common(record.recorded_at, session_id, session_path)
    }

    fn matches_common(&self, timestamp: u64, session_id: &str, session_path: &str) -> bool {
        if self.since.is_some_and(|since| timestamp < since)
            || self.until.is_some_and(|until| timestamp >= until)
        {
            return false;
        }
        if let Some(session) = self.session.as_deref() {
            let session_lower = session.to_ascii_lowercase();
            if !session_id.eq_ignore_ascii_case(session)
                && !session_path.to_ascii_lowercase().contains(&session_lower)
            {
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

#[derive(Debug, Clone, Serialize)]
struct ToolStatsRecord {
    session_id: String,
    session_path: String,
    project: String,
    entry_id: String,
    timestamp: u64,
    tool_call_id: String,
    tool_name: String,
    is_error: bool,
    files_created: usize,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
}

struct StatsDataset {
    filters: StatsFilters,
    tool_records: Vec<ToolStatsRecord>,
    usage_records: Vec<SessionUsageRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum PeriodKind {
    Day,
    Week,
}

#[derive(Debug, Clone, Serialize)]
struct StatsPeriodRow {
    period: String,
    tool_calls: usize,
    tool_errors: usize,
    token_requests: usize,
    total_tokens: u64,
    total_cost: f64,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
}
#[derive(Debug, Clone, Serialize)]
struct StatsNamedRow {
    name: String,
    tool_calls: usize,
    tool_errors: usize,
    token_requests: usize,
    total_tokens: u64,
    total_cost: f64,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
}
#[derive(Debug, Clone, Serialize)]
struct StatsExportJson {
    report: &'static str,
    generated_at: u64,
    filters: StatsFilterSummary,
    summary: StatsSummaryJson,
    tools: Vec<ToolStatsRecord>,
    usage: Vec<SessionUsageRecord>,
}
#[derive(Debug, Clone, Serialize)]
struct WrappedStats {
    generated_at: u64,
    filters: StatsFilterSummary,
    headline: String,
    sessions: usize,
    top_tool: Option<String>,
    busiest_day: Option<String>,
    top_project: Option<String>,
    tool_calls: usize,
    tool_errors: usize,
    total_tokens: u64,
    total_cost: f64,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
}

fn load_stats_dataset(args: &StatsReportArgs) -> Result<StatsDataset, Box<dyn std::error::Error>> {
    let filters = StatsFilters::from_args(args)?;
    let session_dir = Config::session_dir();
    let tool_records = load_tool_records_from_dir(&session_dir)?
        .into_iter()
        .filter(|r| filters.matches_tool(r))
        .collect();
    let usage_records = dedupe_usage_records(&load_usage_records_from_dir(&session_dir)?)
        .into_iter()
        .filter(|r| filters.matches_usage(r))
        .collect();
    Ok(StatsDataset {
        filters,
        tool_records,
        usage_records,
    })
}

fn session_paths(session_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if !session_dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for dir_entry in fs::read_dir(session_dir)? {
        let path = dir_entry?.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn load_tool_records_from_dir(
    session_dir: &Path,
) -> Result<Vec<ToolStatsRecord>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    for path in session_paths(session_dir)? {
        let session = SessionManager::open(&path)?;
        let session_id = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let session_path = path.display().to_string();
        let project = project_from_session(&session, &path);
        for entry in session.entries() {
            if let SessionEntry::Message {
                id,
                message: Message::ToolResult(result),
                ..
            } = entry
            {
                let diff = summarize_tool_diff(&result.details);
                records.push(ToolStatsRecord {
                    session_id: session_id.clone(),
                    session_path: session_path.clone(),
                    project: project.clone(),
                    entry_id: id.clone(),
                    timestamp: result.timestamp,
                    tool_call_id: result.tool_call_id.clone(),
                    tool_name: result.tool_name.clone(),
                    is_error: result.is_error,
                    files_created: diff.files_created,
                    lines_added: diff.lines_added,
                    lines_removed: diff.lines_removed,
                    lines_read: diff.lines_read,
                });
            }
        }
    }
    Ok(records)
}

fn load_usage_records_from_dir(
    session_dir: &Path,
) -> Result<Vec<SessionUsageRecord>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    for path in session_paths(session_dir)? {
        records.extend(SessionManager::open(&path)?.usage_records());
    }
    Ok(records)
}

fn project_from_session(session: &SessionManager, path: &Path) -> String {
    session
        .entries()
        .iter()
        .find_map(|entry| match entry {
            SessionEntry::Header { cwd, .. } => Path::new(cwd)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .or_else(|| Some(cwd.clone())),
            _ => None,
        })
        .unwrap_or_else(|| {
            path.parent()
                .and_then(Path::file_name)
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
}

#[derive(Default)]
struct DiffSummary {
    files_created: usize,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
}

fn summarize_tool_diff(details: &serde_json::Value) -> DiffSummary {
    let mut summary = DiffSummary::default();
    collect_diff_stats(details, &mut summary);
    summary
}

fn collect_diff_stats(value: &serde_json::Value, summary: &mut DiffSummary) {
    match value {
        serde_json::Value::Object(map) => {
            let status = map
                .get("status")
                .or_else(|| map.get("change_type"))
                .or_else(|| map.get("kind"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if matches!(status, "created" | "create" | "added" | "add") {
                summary.files_created += 1;
            }
            for key in ["lines_added", "added_lines", "insertions", "additions"] {
                summary.lines_added +=
                    map.get(key).and_then(|value| value.as_u64()).unwrap_or(0) as usize;
            }
            for key in ["lines_removed", "removed_lines", "deletions", "removals"] {
                summary.lines_removed +=
                    map.get(key).and_then(|value| value.as_u64()).unwrap_or(0) as usize;
            }
            for key in ["lines_read", "read_lines"] {
                summary.lines_read +=
                    map.get(key).and_then(|value| value.as_u64()).unwrap_or(0) as usize;
            }
            for value in map.values() {
                collect_diff_stats(value, summary);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_diff_stats(value, summary);
            }
        }
        _ => {}
    }
}

fn build_stats_summary_json(
    filters: &StatsFilters,
    tool_records: &[ToolStatsRecord],
    usage_records: &[SessionUsageRecord],
) -> StatsSummaryJson {
    let input_tokens: u64 = usage_records.iter().map(|r| u64::from(r.usage.input)).sum();
    let output_tokens: u64 = usage_records
        .iter()
        .map(|r| u64::from(r.usage.output))
        .sum();
    let cache_read_tokens: u64 = usage_records
        .iter()
        .map(|r| u64::from(r.usage.cache_read))
        .sum();
    let cache_write_tokens: u64 = usage_records
        .iter()
        .map(|r| u64::from(r.usage.cache_write))
        .sum();
    StatsSummaryJson {
        report: "summary",
        generated_at: imp_llm::now(),
        filters: filters.summary(),
        sessions: tool_records
            .iter()
            .map(|r| r.session_id.as_str())
            .chain(usage_records.iter().filter_map(|r| r.session_id.as_deref()))
            .collect::<HashSet<_>>()
            .len(),
        tool_calls: tool_records.len(),
        tool_errors: tool_records.iter().filter(|r| r.is_error).count(),
        unique_tools: tool_records
            .iter()
            .map(|r| r.tool_name.as_str())
            .collect::<HashSet<_>>()
            .len(),
        files_created: tool_records.iter().map(|r| r.files_created).sum(),
        lines_added: tool_records.iter().map(|r| r.lines_added).sum(),
        lines_removed: tool_records.iter().map(|r| r.lines_removed).sum(),
        lines_read: tool_records.iter().map(|r| r.lines_read).sum(),
        token_requests: usage_records.len(),
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens: input_tokens + output_tokens + cache_read_tokens + cache_write_tokens,
        total_cost: usage_records
            .iter()
            .filter_map(|r| r.cost.as_ref())
            .map(|c| c.total)
            .sum(),
    }
}

fn build_tool_rows(records: &[ToolStatsRecord]) -> Vec<StatsToolRow> {
    let mut rows: HashMap<String, StatsToolRow> = HashMap::new();
    for record in records {
        let row = rows
            .entry(record.tool_name.clone())
            .or_insert_with(|| StatsToolRow {
                tool: record.tool_name.clone(),
                calls: 0,
                errors: 0,
                files_created: 0,
                lines_added: 0,
                lines_removed: 0,
                lines_read: 0,
            });
        row.calls += 1;
        if record.is_error {
            row.errors += 1;
        }
        row.files_created += record.files_created;
        row.lines_added += record.lines_added;
        row.lines_removed += record.lines_removed;
        row.lines_read += record.lines_read;
    }
    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.tool.cmp(&b.tool)));
    rows
}

fn build_period_rows(dataset: &StatsDataset, kind: PeriodKind) -> Vec<StatsPeriodRow> {
    let mut rows: HashMap<String, StatsPeriodRow> = HashMap::new();
    for record in &dataset.tool_records {
        let key = period_key(record.timestamp, kind);
        let row = rows.entry(key.clone()).or_insert_with(|| StatsPeriodRow {
            period: key,
            tool_calls: 0,
            tool_errors: 0,
            token_requests: 0,
            total_tokens: 0,
            total_cost: 0.0,
            lines_added: 0,
            lines_removed: 0,
            lines_read: 0,
        });
        row.tool_calls += 1;
        if record.is_error {
            row.tool_errors += 1;
        }
        row.lines_added += record.lines_added;
        row.lines_removed += record.lines_removed;
        row.lines_read += record.lines_read;
    }
    for record in &dataset.usage_records {
        let key = period_key(record.recorded_at, kind);
        let row = rows.entry(key.clone()).or_insert_with(|| StatsPeriodRow {
            period: key,
            tool_calls: 0,
            tool_errors: 0,
            token_requests: 0,
            total_tokens: 0,
            total_cost: 0.0,
            lines_added: 0,
            lines_removed: 0,
            lines_read: 0,
        });
        row.token_requests += 1;
        row.total_tokens += usage_total_tokens(record);
        row.total_cost += record.cost.as_ref().map(|c| c.total).unwrap_or(0.0);
    }
    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| b.period.cmp(&a.period));
    rows
}

fn build_project_rows(records: &[ToolStatsRecord]) -> Vec<StatsNamedRow> {
    build_named_tool_rows(records, |r| r.project.clone())
}

fn build_session_rows(dataset: &StatsDataset) -> Vec<StatsNamedRow> {
    let mut rows = build_named_tool_rows(&dataset.tool_records, |r| r.session_id.clone())
        .into_iter()
        .map(|r| (r.name.clone(), r))
        .collect::<HashMap<_, _>>();
    for record in &dataset.usage_records {
        let name = record
            .session_id
            .clone()
            .or_else(|| record.session_path.clone())
            .unwrap_or_else(|| "unknown-session".into());
        let row = rows.entry(name.clone()).or_insert_with(|| StatsNamedRow {
            name,
            tool_calls: 0,
            tool_errors: 0,
            token_requests: 0,
            total_tokens: 0,
            total_cost: 0.0,
            lines_added: 0,
            lines_removed: 0,
            lines_read: 0,
        });
        row.token_requests += 1;
        row.total_tokens += usage_total_tokens(record);
        row.total_cost += record.cost.as_ref().map(|c| c.total).unwrap_or(0.0);
    }
    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        b.tool_calls
            .cmp(&a.tool_calls)
            .then_with(|| b.total_tokens.cmp(&a.total_tokens))
    });
    rows
}

fn build_named_tool_rows<F>(records: &[ToolStatsRecord], name_for: F) -> Vec<StatsNamedRow>
where
    F: Fn(&ToolStatsRecord) -> String,
{
    let mut rows: HashMap<String, StatsNamedRow> = HashMap::new();
    for record in records {
        let name = name_for(record);
        let row = rows.entry(name.clone()).or_insert_with(|| StatsNamedRow {
            name,
            tool_calls: 0,
            tool_errors: 0,
            token_requests: 0,
            total_tokens: 0,
            total_cost: 0.0,
            lines_added: 0,
            lines_removed: 0,
            lines_read: 0,
        });
        row.tool_calls += 1;
        if record.is_error {
            row.tool_errors += 1;
        }
        row.lines_added += record.lines_added;
        row.lines_removed += record.lines_removed;
        row.lines_read += record.lines_read;
    }
    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        b.tool_calls
            .cmp(&a.tool_calls)
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

fn build_wrapped(dataset: &StatsDataset) -> WrappedStats {
    let summary = build_stats_summary_json(
        &dataset.filters,
        &dataset.tool_records,
        &dataset.usage_records,
    );
    let top_tool = build_tool_rows(&dataset.tool_records)
        .first()
        .map(|r| r.tool.clone());
    let busiest_day = build_period_rows(dataset, PeriodKind::Day)
        .first()
        .map(|r| r.period.clone());
    let top_project = build_project_rows(&dataset.tool_records)
        .first()
        .map(|r| r.name.clone());
    let headline = match top_tool.as_deref() {
        Some(tool) => format!("Your top imp tool was {tool}."),
        None => "No local imp tool activity found yet.".to_string(),
    };
    WrappedStats {
        generated_at: imp_llm::now(),
        filters: dataset.filters.summary(),
        headline,
        sessions: summary.sessions,
        top_tool,
        busiest_day,
        top_project,
        tool_calls: summary.tool_calls,
        tool_errors: summary.tool_errors,
        total_tokens: summary.total_tokens,
        total_cost: summary.total_cost,
        lines_added: summary.lines_added,
        lines_removed: summary.lines_removed,
        lines_read: summary.lines_read,
    }
}

fn build_stats_export_json(dataset: &StatsDataset) -> StatsExportJson {
    StatsExportJson {
        report: "export",
        generated_at: imp_llm::now(),
        filters: dataset.filters.summary(),
        summary: build_stats_summary_json(
            &dataset.filters,
            &dataset.tool_records,
            &dataset.usage_records,
        ),
        tools: dataset.tool_records.clone(),
        usage: dataset.usage_records.clone(),
    }
}

fn usage_total_tokens(record: &SessionUsageRecord) -> u64 {
    u64::from(record.usage.input)
        + u64::from(record.usage.output)
        + u64::from(record.usage.cache_read)
        + u64::from(record.usage.cache_write)
}
fn period_key(timestamp: u64, kind: PeriodKind) -> String {
    let days = (timestamp / 86_400) as i64;
    match kind {
        PeriodKind::Day => format_utc_day(days),
        PeriodKind::Week => format_utc_week(days),
    }
}
fn format_utc_day(days_since_epoch: i64) -> String {
    let (y, m, d) = civil_from_days(days_since_epoch);
    format!("{y:04}-{m:02}-{d:02}")
}
fn format_utc_week(days_since_epoch: i64) -> String {
    let thursday = days_since_epoch + 3;
    let (year, _, _) = civil_from_days(thursday);
    let jan_4 = days_from_civil(year, 1, 4);
    let week1_monday = jan_4 - ((jan_4 + 3).rem_euclid(7));
    let monday = days_since_epoch - ((days_since_epoch + 3).rem_euclid(7));
    let week = ((monday - week1_monday) / 7) + 1;
    format!("{year:04}-W{week:02}")
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

fn print_stats_summary_table(s: &StatsSummaryJson) {
    println!("Imp stats");
    println!("  sessions       {}", s.sessions);
    println!("  tool calls     {}", s.tool_calls);
    println!("  tool errors    {}", s.tool_errors);
    println!("  unique tools   {}", s.unique_tools);
    println!("  token requests {}", s.token_requests);
    println!("  total tokens   {}", s.total_tokens);
    println!("  total cost     ${:.4}", s.total_cost);
    println!("  files created  {}", s.files_created);
    println!("  lines read     {}", s.lines_read);
    println!("  lines added    {}", s.lines_added);
    println!("  lines removed  {}", s.lines_removed);
}
fn print_tool_rows(rows: &[StatsToolRow]) {
    println!(
        "{:<24} {:>8} {:>8} {:>8} {:>10} {:>10}",
        "tool", "calls", "errors", "created", "+lines", "-lines"
    );
    for row in rows {
        println!(
            "{:<24} {:>8} {:>8} {:>8} {:>10} {:>10}",
            truncate_name(&row.tool, 24),
            row.calls,
            row.errors,
            row.files_created,
            row.lines_added,
            row.lines_removed
        );
    }
}
fn print_period_rows(kind: PeriodKind, rows: &[StatsPeriodRow]) {
    let label = match kind {
        PeriodKind::Day => "day",
        PeriodKind::Week => "week",
    };
    println!(
        "{:<12} {:>8} {:>8} {:>8} {:>12} {:>10} {:>10}",
        label, "tools", "errors", "tokens", "cost", "+lines", "-lines"
    );
    for row in rows {
        println!(
            "{:<12} {:>8} {:>8} {:>8} ${:>11.4} {:>10} {:>10}",
            row.period,
            row.tool_calls,
            row.tool_errors,
            row.total_tokens,
            row.total_cost,
            row.lines_added,
            row.lines_removed
        );
    }
}
fn print_named_rows(label: &str, rows: &[StatsNamedRow]) {
    println!(
        "{:<28} {:>8} {:>8} {:>8} {:>12} {:>10} {:>10}",
        label, "tools", "errors", "tokens", "cost", "+lines", "-lines"
    );
    for row in rows {
        println!(
            "{:<28} {:>8} {:>8} {:>8} ${:>11.4} {:>10} {:>10}",
            truncate_name(&row.name, 28),
            row.tool_calls,
            row.tool_errors,
            row.total_tokens,
            row.total_cost,
            row.lines_added,
            row.lines_removed
        );
    }
}
fn print_wrapped(w: &WrappedStats) {
    println!("Imp wrapped");
    println!("  {}", w.headline);
    if let Some(day) = &w.busiest_day {
        println!("  Busiest day: {day}");
    }
    if let Some(project) = &w.top_project {
        println!("  Top project: {project}");
    }
    println!("  Sessions: {}", w.sessions);
    println!("  Tool calls: {} ({} errors)", w.tool_calls, w.tool_errors);
    println!("  Tokens: {} (${:.4})", w.total_tokens, w.total_cost);
    println!("  Code churn: +{} -{}", w.lines_added, w.lines_removed);
    println!("  Lines read: {}", w.lines_read);
}
fn truncate_name(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max - 1).collect();
    out.push('…');
    out
}
fn print_json_pretty<T: Serialize>(value: &T) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summarizes_nested_diff_details() {
        let details = serde_json::json!({"files":[{"status":"created","lines_added":4},{"status":"modified","insertions":10,"deletions":3},{"change_type":"add","added_lines":2,"removed_lines":1},{"status":"read","lines_read":7}]});
        let summary = summarize_tool_diff(&details);
        assert_eq!(summary.files_created, 2);
        assert_eq!(summary.lines_added, 16);
        assert_eq!(summary.lines_removed, 4);
        assert_eq!(summary.lines_read, 7);
    }
    #[test]
    fn tool_rows_group_and_sort_by_calls() {
        let records = vec![
            record("bash", false, 0, 1, 0),
            record("edit", true, 1, 3, 2),
            record("bash", true, 0, 0, 0),
        ];
        let rows = build_tool_rows(&records);
        assert_eq!(rows[0].tool, "bash");
        assert_eq!(rows[0].calls, 2);
        assert_eq!(rows[0].errors, 1);
        assert_eq!(rows[1].tool, "edit");
        assert_eq!(rows[1].files_created, 1);
    }
    #[test]
    fn period_keys_are_stable() {
        assert_eq!(period_key(0, PeriodKind::Day), "1970-01-01");
        assert_eq!(period_key(0, PeriodKind::Week), "1970-W01");
    }
    fn record(
        tool_name: &str,
        is_error: bool,
        files_created: usize,
        lines_added: usize,
        lines_removed: usize,
    ) -> ToolStatsRecord {
        ToolStatsRecord {
            session_id: "session".into(),
            session_path: "session.jsonl".into(),
            project: "project".into(),
            entry_id: format!("{tool_name}-{lines_added}"),
            timestamp: 1,
            tool_call_id: "call".into(),
            tool_name: tool_name.into(),
            is_error,
            files_created,
            lines_added,
            lines_removed,
            lines_read: 0,
        }
    }
}
