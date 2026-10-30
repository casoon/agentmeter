//! Text and JSON output.

use agentmeter_core::findings::{Evaluation, Level};
use agentmeter_core::format::{count, tokens};
use agentmeter_core::model::{DataQuality, ReadStats};
use agentmeter_core::report::{ProjectRow, SessionRow, Summary};
use serde_json::{Value, json};

/// Rows shown per table; the JSON output carries all of them.
const TABLE_ROWS: usize = 20;

/// ANSI styling, switched off for pipes and `NO_COLOR`.
#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
    fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }
    fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }
}

/// What a run selected and measured.
pub struct Report<'a> {
    pub tool: &'a str,
    pub label: &'a str,
    pub period: String,
    pub project: Option<&'a str>,
    pub quality: &'a DataQuality,
    pub summary: Option<&'a Summary>,
    pub sessions: Option<&'a [SessionRow]>,
    pub projects: Option<&'a [ProjectRow]>,
    pub findings: Option<&'a Evaluation>,
}

fn pct(value: f64) -> String {
    format!("{value:.0} %")
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_owned()
    } else {
        let mut clipped: String = text.chars().take(width - 1).collect();
        clipped.push('…');
        clipped
    }
}

fn heading(style: Style, title: &str) -> String {
    format!("\n{}\n", style.bold(title))
}

pub fn text(report: &Report, style: Style) -> String {
    let mut out = String::new();
    let scope = report
        .project
        .map_or(String::new(), |p| format!(" · project \"{p}\""));
    out += &format!(
        "{} · {} · {}{scope}\n",
        style.bold("agentmeter"),
        report.label,
        report.period
    );
    if let Some(summary) = report.summary {
        out += &summary_text(summary, style);
    }
    if let Some(findings) = report.findings {
        out += &findings_text(findings, style);
    }
    if let Some(sessions) = report.sessions {
        out += &sessions_text(sessions, style);
    }
    if let Some(projects) = report.projects {
        out += &projects_text(projects, style);
    }
    out += &read_text(report.quality, style);
    out
}

fn summary_text(s: &Summary, style: Style) -> String {
    let mut out = heading(style, "SUMMARY");
    let sub = if s.requests == 0 {
        0.0
    } else {
        s.sub_agent_requests as f64 * 100.0 / s.requests as f64
    };
    out += &format!(
        "  Requests      {}  (sub-agents {})\n",
        count(s.requests),
        pct(sub)
    );
    out += &format!(
        "  Sessions      {}  in {} projects\n",
        count(s.sessions),
        count(s.projects)
    );
    out += &format!(
        "  Input         {}  cache read {} · written {} (5 min {} · 1 h {})\n",
        tokens(s.input),
        pct(s.cache_read_pct),
        tokens(s.cache_write_5m + s.cache_write_1h),
        tokens(s.cache_write_5m),
        tokens(s.cache_write_1h)
    );
    out += &format!("  Output        {}\n", tokens(s.output));
    let c = &s.compactions;
    if c.total == 0 {
        out += "  Compactions   none\n";
    } else {
        out += &format!(
            "  Compactions   {} (auto {}, manual {}, trigger unknown {}) · before: avg {}, min {}, max {}\n",
            count(c.total),
            count(c.auto),
            count(c.manual),
            count(c.trigger_unknown),
            tokens(c.avg_tokens_before),
            tokens(c.min_tokens_before),
            tokens(c.max_tokens_before)
        );
    }
    if !s.days.is_empty() {
        out += &heading(style, "DAYS (UTC)");
        out += &style.dim("  Day          Requests     Input   Cache  Compactions");
        out += "\n";
        for d in &s.days {
            out += &format!(
                "  {}  {:>8}  {:>8}  {:>5}  {:>11}\n",
                d.date,
                count(d.requests),
                tokens(d.input),
                pct(d.cache_read_pct),
                d.compactions
            );
        }
    }
    if !s.models.is_empty() {
        out += &heading(style, "MODELS");
        for m in s.models.iter().take(TABLE_ROWS) {
            let name = if m.model.is_empty() {
                "(unknown)"
            } else {
                &m.model
            };
            out += &format!("  {:<28} {:>8}\n", clip(name, 28), count(m.requests));
        }
    }
    out
}

fn findings_text(evaluation: &Evaluation, style: Style) -> String {
    let mut out = heading(style, "FINDINGS");
    match evaluation.verdict {
        "no_data" => {
            out += "  No analysis possible: no requests with usage in the selected period.\n"
        }
        "none" => {
            out += "  No findings. Every rule that could be measured found nothing to report.\n"
        }
        _ => {}
    }
    for f in &evaluation.findings {
        let (mark, code) = match f.level {
            Level::Finding => ("✗", "31"),
            Level::Notice => ("!", "33"),
            Level::Info => ("i", "36"),
        };
        out += &format!(
            "  {} {} {}\n",
            style.paint(code, mark),
            style.bold(&format!("{} v{}", f.rule, f.rule_version)),
            style.dim(&format!("{} · {}", f.level.label(), f.certainty))
        );
        out += &format!("    {}\n", f.message);
        let m = &f.measure;
        out += &style.dim(&format!(
            "    measure: {} = {} {} of {} {} (sample {} {})\n",
            m.name,
            m.value,
            m.unit,
            count(m.denominator),
            m.denominator_unit,
            count(m.sample),
            m.sample_unit
        ));
        for e in &f.evidence {
            out += &style.dim(&format!("    evidence: {} · {}\n", e.file, e.event));
        }
        for c in &f.counterexamples {
            out += &style.dim(&format!("    but: {c}\n"));
        }
        out += &format!("    {} {}\n", style.bold("next:"), f.next_step);
    }
    // Every rule appears once per source: observed above, the others as checked or not measurable.
    for r in evaluation.rules.iter().filter(|r| r.status != "observed") {
        let line = match (r.status, &r.message, &r.reason) {
            ("checked_none", Some(message), _) => format!("checked, nothing to report: {message}"),
            (_, _, reason) => format!("not measurable: {}", reason.as_deref().unwrap_or("")),
        };
        out += &style.dim(&format!("  {} v{} {line}\n", r.rule, r.rule_version));
    }
    out
}

fn sessions_text(rows: &[SessionRow], style: Style) -> String {
    let mut out = heading(
        style,
        &format!(
            "SESSIONS (top {} of {} by input)",
            rows.len().min(TABLE_ROWS),
            count(rows.len() as u64)
        ),
    );
    out += &style
        .dim("  Session   Project            Requests   Span     Peak   Input  Cache  Compact.");
    out += "\n";
    for r in rows.iter().take(TABLE_ROWS) {
        out += &format!(
            "  {:<8}  {:<18} {:>8}  {:>5.1} h  {:>6}  {:>6}  {:>5}  {:>6}\n",
            clip(&r.session, 8),
            clip(&r.project, 18),
            count(r.requests),
            r.hours,
            tokens(r.peak_context),
            tokens(r.input),
            pct(r.cache_read_pct),
            r.compactions
        );
    }
    out
}

fn projects_text(rows: &[ProjectRow], style: Style) -> String {
    let mut out = heading(
        style,
        &format!(
            "PROJECTS (top {} of {} by input)",
            rows.len().min(TABLE_ROWS),
            count(rows.len() as u64)
        ),
    );
    out += &style.dim("  Project                  Sessions  Requests   Input  Share  Cache");
    out += "\n";
    for r in rows.iter().take(TABLE_ROWS) {
        out += &format!(
            "  {:<24} {:>8}  {:>8}  {:>6}  {:>5}  {:>5}\n",
            clip(&r.project, 24),
            count(r.sessions),
            count(r.requests),
            tokens(r.input),
            pct(r.input_share_pct),
            pct(r.cache_read_pct)
        );
    }
    out
}

fn read_text(quality: &DataQuality, style: Style) -> String {
    let stats: &ReadStats = &quality.read;
    let mut out = String::new();
    let versions = match (&stats.first_version, &stats.last_version) {
        (Some(a), Some(b)) if a != b => format!(" · version {a} – {b}"),
        (Some(a), _) => format!(" · version {a}"),
        _ => String::new(),
    };
    let observed = match (&quality.observed_from, &quality.observed_to) {
        (Some(from), Some(to)) => format!("observed {from} – {to}"),
        _ => "no requests observed".to_owned(),
    };
    out += &style.dim(&format!(
        "\nData: {observed}\n  Read {} files, {} lines, {} requests{versions}\n",
        count(stats.files),
        count(stats.lines),
        count(quality.requests)
    ));
    let mut notes = Vec::new();
    if stats.files_without_usage > 0 {
        notes.push(format!(
            "{} files without usage",
            count(stats.files_without_usage)
        ));
    }
    if stats.placeholder_version_entries > 0 {
        notes.push(format!(
            "{} entries carry the placeholder version 0.0.0, left out of the version range",
            count(stats.placeholder_version_entries)
        ));
    }
    if stats.compressed_files_read
        + stats.compressed_files_unreadable
        + stats.compressed_files_shadowed
        > 0
    {
        notes.push(format!(
            "compressed files (.zst): {} read, {} unreadable (damaged or over the limits), {} skipped because a plain copy exists",
            count(stats.compressed_files_read),
            count(stats.compressed_files_unreadable),
            count(stats.compressed_files_shadowed)
        ));
    }
    if !stats.cache_write_measured && quality.requests > 0 {
        notes.push("cache writes not recorded in this source: not measured".to_owned());
    }
    if stats.usage_delta_mismatches > 0 {
        notes.push(format!(
            "{} usage events whose change of the running total differs from the event's own value",
            count(stats.usage_delta_mismatches)
        ));
    }
    if stats.skipped_lines > 0 {
        notes.push(format!(
            "{} unreadable entries skipped",
            count(stats.skipped_lines)
        ));
    }
    if stats.requests_without_model > 0 {
        notes.push(format!(
            "{} requests without a model name",
            count(stats.requests_without_model)
        ));
    }
    if stats.compactions_without_post_tokens > 0 {
        notes.push(format!(
            "{} compactions without size after compaction",
            count(stats.compactions_without_post_tokens)
        ));
    }
    if stats.requests_flat_cache > 0 {
        notes.push(format!(
            "{} usage lines without cache lifetime split (5 min / 1 h)",
            count(stats.requests_flat_cache)
        ));
    }
    if stats.duplicates_merged > 0 || stats.replays_dropped > 0 {
        notes.push(format!(
            "{} duplicate lines merged, {} sub-agent replays dropped",
            count(stats.duplicates_merged),
            count(stats.replays_dropped)
        ));
    }
    if !stats.unknown_types.is_empty() {
        notes.push(format!(
            "unknown entry types: {}",
            list(&stats.unknown_types)
        ));
    }
    if !stats.unknown_usage_fields.is_empty() {
        notes.push(format!(
            "unknown usage fields: {}",
            list(&stats.unknown_usage_fields)
        ));
    }
    for note in notes {
        out += &style.dim(&format!("  {note}"));
        out += "\n";
    }
    out
}

fn list(map: &std::collections::BTreeMap<String, u64>) -> String {
    map.iter()
        .map(|(name, n)| format!("{name} ({})", count(*n)))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn json(reports: &[Report]) -> String {
    let sources: Vec<Value> = reports.iter().map(json_source).collect();
    serde_json::to_string_pretty(&json!({ "schema_version": SCHEMA_VERSION, "sources": sources }))
        .expect("serialisable")
}

/// Version of the JSON layout documented in `docs/reference/json.md`. Any change to it is a
/// changelog entry.
pub const SCHEMA_VERSION: u32 = 1;

/// Result of `--check data-quality` as text: one line per source, reasons below.
pub fn check_text(results: &[(&crate::Source, Vec<String>)]) -> String {
    let mut out = String::new();
    for (source, reasons) in results {
        if reasons.is_empty() {
            out += &format!("{}: data quality ok\n", source.label);
        } else {
            out += &format!("{}: data quality check failed\n", source.label);
            for reason in reasons {
                out += &format!("  {reason}\n");
            }
        }
    }
    out
}

/// Result of `--check data-quality` as JSON.
pub fn check_json(results: &[(&crate::Source, Vec<String>)]) -> String {
    let sources: Vec<Value> = results
        .iter()
        .map(|(source, reasons)| {
            json!({ "tool": source.tool, "passed": reasons.is_empty(), "reasons": reasons })
        })
        .collect();
    let passed = results.iter().all(|(_, reasons)| reasons.is_empty());
    serde_json::to_string_pretty(&json!({
        "schema_version": SCHEMA_VERSION,
        "check": "data-quality",
        "passed": passed,
        "sources": sources,
    }))
    .expect("serialisable")
}

fn json_source(report: &Report) -> Value {
    let mut root = json!({
        "tool": report.tool,
        "period": report.period,
        "project": report.project,
        "data_quality": report.quality,
    });
    let map = root.as_object_mut().expect("object literal");
    let mut put = |key: &str, value: Value| {
        map.insert(key.to_owned(), value);
    };
    if let Some(v) = report.summary {
        put("summary", json!(v));
    }
    if let Some(v) = report.sessions {
        put("sessions", json!(v));
    }
    if let Some(v) = report.projects {
        put("projects", json!(v));
    }
    if let Some(v) = report.findings {
        put("verdict", json!(v.verdict));
        put("findings", json!(v.findings));
        put("rules", json!(v.rules));
    }
    root
}
