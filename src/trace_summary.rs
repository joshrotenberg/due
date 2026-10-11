//! Offline JSONL inspection. No provider, tracing subscriber, or subprocess is started.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::ExitCode,
};

use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cli::Output;

const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_RUNS: usize = 128;
const MAX_EVENT_TYPES: usize = 256;
const MAX_OPEN_ACTIVITIES: usize = 4096;

#[derive(Debug, Args)]
pub struct TraceArgs {
    #[command(subcommand)]
    command: TraceCommand,
}

#[derive(Debug, Subcommand)]
enum TraceCommand {
    /// Summarize a saved JSONL trace without starting a provider
    Summarize {
        /// JSONL trace file (read-only; no provider or network access)
        file: PathBuf,
        #[arg(long, value_enum, default_value = "text")]
        output: Output,
    },
}

#[derive(Default, Serialize)]
struct Delivery {
    /// Forward sequence gaps observed, not a claim about uniquely lost records.
    sequence_gap_records: u64,
    non_increasing_sequences: u64,
    reported_dropped_records: u64,
}

#[derive(Default, Serialize)]
struct Activities {
    started: u64,
    completed: u64,
    failed: u64,
    unmatched_starts: BTreeSet<String>,
    completions_without_observed_start: u64,
    starts_without_id: u64,
    completions_without_id: u64,
    duplicate_starts: u64,
    completion_status_counts: BTreeMap<String, u64>,
    by_kind: BTreeMap<String, ActivityCounts>,
}

#[derive(Default, Serialize)]
struct ActivityCounts {
    started: u64,
    completed: u64,
    completion_status_counts: BTreeMap<String, u64>,
}

#[derive(Default, Serialize)]
struct ReportedCounts {
    activities_started: u64,
    activities_completed: u64,
    activities_failed: u64,
    messages: u64,
}

#[derive(Serialize)]
struct RunSummary {
    run_id: String,
    provider: String,
    session_id: Option<String>,
    session_id_changes: u64,
    pid: Option<u64>,
    pgid: Option<u64>,
    records: u64,
    event_counts: BTreeMap<String, u64>,
    elapsed_ms: u64,
    status: Option<String>,
    exit_code: Option<i64>,
    terminal_records: u64,
    terminal_conflict: bool,
    #[serde(skip)]
    observed_terminal: Option<(String, i64)>,
    run_started_observed: bool,
    delivery: Delivery,
    activities: Activities,
    messages: u64,
    reported_counts: ReportedCounts,
    #[serde(skip)]
    highest_seq: u64,
}

impl RunSummary {
    fn new(record: &Value) -> Self {
        Self {
            run_id: record["run_id"].as_str().unwrap().into(),
            provider: record["provider"].as_str().unwrap().into(),
            session_id: None,
            session_id_changes: 0,
            pid: None,
            pgid: None,
            records: 0,
            event_counts: BTreeMap::new(),
            elapsed_ms: 0,
            status: None,
            exit_code: None,
            terminal_records: 0,
            terminal_conflict: false,
            observed_terminal: None,
            run_started_observed: false,
            delivery: Delivery::default(),
            activities: Activities::default(),
            messages: 0,
            reported_counts: ReportedCounts::default(),
            highest_seq: 0,
        }
    }

    fn observe(&mut self, value: &Value) -> Result<(), String> {
        if value["provider"].as_str() != Some(&self.provider) {
            return Err("a run_id has conflicting providers; use distinct run IDs".into());
        }
        let event = value["event"].as_str().unwrap();
        if self.event_counts.len() >= MAX_EVENT_TYPES && !self.event_counts.contains_key(event) {
            return Err("more than 256 event types in one run; split the trace".into());
        }
        *self.event_counts.entry(event.into()).or_default() += 1;
        self.records += 1;
        let seq = value["seq"].as_u64().unwrap();
        if seq <= self.highest_seq {
            self.delivery.non_increasing_sequences += 1;
        } else {
            self.delivery.sequence_gap_records = self
                .delivery
                .sequence_gap_records
                .saturating_add(seq - self.highest_seq - 1);
            self.highest_seq = seq;
        }
        self.elapsed_ms = self.elapsed_ms.max(value["elapsed_ms"].as_u64().unwrap());
        self.delivery.reported_dropped_records = self
            .delivery
            .reported_dropped_records
            .max(value["dropped_records"].as_u64().unwrap());
        self.reported_counts.activities_started = self
            .reported_counts
            .activities_started
            .max(value["activities_started"].as_u64().unwrap());
        self.reported_counts.activities_completed = self
            .reported_counts
            .activities_completed
            .max(value["activities_completed"].as_u64().unwrap());
        self.reported_counts.activities_failed = self
            .reported_counts
            .activities_failed
            .max(value["activities_failed"].as_u64().unwrap());
        self.reported_counts.messages = self
            .reported_counts
            .messages
            .max(value["messages"].as_u64().unwrap());
        if let Some(session) = value.get("session_id").and_then(Value::as_str) {
            if self.session_id.as_deref().is_some_and(|old| old != session) {
                self.session_id_changes += 1;
            }
            self.session_id = Some(session.into());
        }
        self.pid = value.get("pid").and_then(Value::as_u64).or(self.pid);
        self.pgid = value.get("pgid").and_then(Value::as_u64).or(self.pgid);
        let id = value.get("activity_id").and_then(Value::as_str);
        if matches!(event, "activity_started" | "activity_completed") {
            let kind = value["activity"].as_str().unwrap();
            if self.activities.by_kind.len() >= MAX_EVENT_TYPES
                && !self.activities.by_kind.contains_key(kind)
            {
                return Err("more than 256 activity kinds in one run; split the trace".into());
            }
            let counts = self.activities.by_kind.entry(kind.into()).or_default();
            if event == "activity_started" {
                counts.started += 1;
            } else {
                counts.completed += 1;
                *counts
                    .completion_status_counts
                    .entry(value["status"].as_str().unwrap().into())
                    .or_default() += 1;
            }
        }
        match event {
            "run_started" => self.run_started_observed = true,
            "run_finished" => {
                self.terminal_records += 1;
                let terminal = (
                    value["status"].as_str().unwrap().to_owned(),
                    value["exit_code"].as_i64().unwrap(),
                );
                if self
                    .observed_terminal
                    .as_ref()
                    .is_some_and(|old| old != &terminal)
                {
                    self.terminal_conflict = true;
                }
                self.observed_terminal = Some(terminal.clone());
                if self.terminal_conflict {
                    self.status = None;
                    self.exit_code = None;
                } else {
                    self.status = Some(terminal.0);
                    self.exit_code = Some(terminal.1);
                }
            }
            "message_completed" => self.messages += 1,
            "activity_started" => {
                self.activities.started += 1;
                if let Some(id) = id {
                    if self.activities.unmatched_starts.contains(id) {
                        self.activities.duplicate_starts += 1;
                    } else if self.activities.unmatched_starts.len() >= MAX_OPEN_ACTIVITIES {
                        return Err(
                            "more than 4096 open activity IDs in one run; split the trace".into(),
                        );
                    } else {
                        self.activities.unmatched_starts.insert(id.into());
                    }
                } else {
                    self.activities.starts_without_id += 1;
                }
            }
            "activity_completed" => {
                self.activities.completed += 1;
                *self
                    .activities
                    .completion_status_counts
                    .entry(value["status"].as_str().unwrap().into())
                    .or_default() += 1;
                self.activities.failed += u64::from(value["status"] == "failed");
                if let Some(id) = id {
                    if !self.activities.unmatched_starts.remove(id) {
                        self.activities.completions_without_observed_start += 1;
                    }
                } else {
                    self.activities.completions_without_id += 1;
                }
            }
            _ => (),
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Summary {
    schema_version: u32,
    records: u64,
    runs: Vec<RunSummary>,
}

fn identifier(value: &Value, field: &str) -> Result<(), String> {
    if value[field].as_str().is_some_and(|text| {
        !text.is_empty()
            && text.len() <= 160
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-.:/".contains(c))
    }) {
        Ok(())
    } else {
        Err(format!(
            "{field} must be a nonempty identifier of at most 160 ASCII bytes"
        ))
    }
}

fn unsigned(value: &Value, field: &str) -> Result<(), String> {
    value[field]
        .as_u64()
        .map(|_| ())
        .ok_or_else(|| format!("{field} must be a nonnegative integer"))
}

fn validate(value: &Value) -> Result<(), String> {
    if !value.is_object() || value["schema_version"].as_u64() != Some(1) {
        return Err("expected a trace object with schema_version 1".into());
    }
    for field in ["run_id", "event", "provider"] {
        identifier(value, field)?;
    }
    if !matches!(value["provider"].as_str(), Some("claude" | "codex")) {
        return Err("provider must be claude or codex".into());
    }
    for field in [
        "seq",
        "elapsed_ms",
        "timestamp_ms",
        "idle_ms",
        "activities_started",
        "activities_completed",
        "activities_failed",
        "messages",
        "dropped_records",
    ] {
        unsigned(value, field)?;
    }
    if value["seq"] == 0 {
        return Err("seq must be positive".into());
    }
    for field in ["session_id", "activity_id", "tool", "activity"] {
        if value.get(field).is_some() {
            identifier(value, field)?;
        }
    }
    for field in [
        "pid",
        "pgid",
        "activity_duration_ms",
        "message_bytes",
        "duration_ms",
        "prompt_bytes",
    ] {
        if value.get(field).is_some() {
            unsigned(value, field)?;
        }
    }
    if let Some(detail) = value.get("detail")
        && (!detail.is_object()
            || detail["text"].as_str().is_none_or(|s| s.len() > 240)
            || !detail["truncated"].is_boolean())
    {
        return Err(
            "detail must contain text (at most 240 UTF-8 bytes) and a truncated boolean".into(),
        );
    }
    match value["event"].as_str().unwrap() {
        "run_started" => {
            for field in ["resumed", "ephemeral", "dry_run"] {
                if !value[field].is_boolean() {
                    return Err(format!("{field} must be a boolean"));
                }
            }
        }
        "prompt_ready" => unsigned(value, "prompt_bytes")?,
        "process_started" => unsigned(value, "pid")?,
        "session_started" => identifier(value, "session_id")?,
        "activity_started" | "activity_detail" => identifier(value, "activity")?,
        "activity_completed" => {
            identifier(value, "activity")?;
            if !matches!(
                value["status"].as_str(),
                Some("succeeded" | "failed" | "cancelled" | "unknown")
            ) {
                return Err("activity completion needs a valid status".into());
            }
        }
        "message_completed" => unsigned(value, "message_bytes")?,
        "run_finished" => {
            if !matches!(
                value["status"].as_str(),
                Some("planned" | "succeeded" | "failed" | "cancelled" | "timed_out")
            ) || value["exit_code"].as_i64().is_none()
            {
                return Err("run_finished needs a valid status and integer exit_code".into());
            }
            unsigned(value, "duration_ms")?;
        }
        _ => (), // New native/progress events remain visible in event_counts.
    }
    Ok(())
}

fn summarize(reader: impl BufRead) -> Result<Summary, String> {
    let mut reader = reader;
    let mut runs = BTreeMap::new();
    let mut records = 0;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .by_ref()
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("read trace: {error}"))?;
        if read == 0 {
            break;
        }
        let line_number = records + 1;
        if line.len() > MAX_LINE_BYTES {
            return Err(format!(
                "line {line_number} exceeds the 1 MiB trace-line limit; use a Due JSONL trace"
            ));
        }
        let value: Value = serde_json::from_slice(&line).map_err(|error| {
            format!("line {line_number} is invalid JSON: {error}; use a Due JSONL trace")
        })?;
        validate(&value).map_err(|error| format!("line {line_number}: {error}"))?;
        let id = value["run_id"].as_str().unwrap();
        if runs.len() >= MAX_RUNS && !runs.contains_key(id) {
            return Err("more than 128 runs; split the trace into smaller files".into());
        }
        let run = runs
            .entry(id.to_owned())
            .or_insert_with(|| RunSummary::new(&value));
        run.observe(&value)
            .map_err(|error| format!("line {line_number}: {error}"))?;
        records += 1;
    }
    Ok(Summary {
        schema_version: 1,
        records,
        runs: runs.into_values().collect(),
    })
}

pub fn execute(args: &TraceArgs) -> ExitCode {
    let TraceCommand::Summarize { file, output } = &args.command;
    let result = File::open(file)
        .map_err(|error| (1, format!("cannot open trace {file:?}: {error}")))
        .and_then(|file| summarize(BufReader::new(file)).map_err(|message| (2, message)));
    let mut stdout = io::stdout().lock();
    let (code, written) = match result {
        Ok(summary) => (
            0,
            match output {
                Output::Json => serde_json::to_writer(&mut stdout, &summary)
                    .map_err(io::Error::other)
                    .and_then(|_| writeln!(stdout)),
                Output::Text => write_text(&mut stdout, &summary),
            },
        ),
        Err((code, message)) => (
            code,
            match output {
                Output::Json => {
                    let value = json!({"schema_version":1,"error":{"kind":if code == 1 {"io"} else {"invalid_input"},"message":message}});
                    serde_json::to_writer(&mut stdout, &value)
                        .map_err(io::Error::other)
                        .and_then(|_| writeln!(stdout))
                }
                Output::Text => writeln!(io::stderr().lock(), "due: {message}"),
            },
        ),
    };
    if let Err(error) = written.and_then(|_| stdout.flush()) {
        eprintln!("due: write trace summary: {error}");
        return ExitCode::from(1);
    }
    ExitCode::from(code)
}

fn write_text(out: &mut impl Write, summary: &Summary) -> io::Result<()> {
    writeln!(
        out,
        "{} records across {} runs",
        summary.records,
        summary.runs.len()
    )?;
    for run in &summary.runs {
        writeln!(
            out,
            "{} {}: {} ({} terminal records, {} ms)",
            run.provider,
            run.run_id,
            run.status
                .as_deref()
                .unwrap_or("unknown: terminal record absent"),
            run.terminal_records,
            run.elapsed_ms
        )?;
        writeln!(
            out,
            "  activities: {} started, {} completed, {} failed; {} open IDs, {} completions without observed starts; {} messages",
            run.activities.started,
            run.activities.completed,
            run.activities.failed,
            run.activities.unmatched_starts.len(),
            run.activities.completions_without_observed_start,
            run.messages
        )?;
        writeln!(
            out,
            "  delivery: {} sequence-gap records observed, {} non-increasing sequences, {} reported drops",
            run.delivery.sequence_gap_records,
            run.delivery.non_increasing_sequences,
            run.delivery.reported_dropped_records
        )?;
    }
    Ok(())
}
