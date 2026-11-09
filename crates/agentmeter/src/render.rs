//! Text and JSON output.

use agentmeter_core::findings::{Evaluation, Finding, Part};
use agentmeter_core::format::{count, tokens};
use agentmeter_core::instructions::{InstructionScan, LoadState};
use agentmeter_core::model::{DataQuality, PROBE_EMPTY_PLACE, Probe, ProbeItem, ReadStats};
use agentmeter_core::report::{ProjectRow, SessionRow, Summary};
use agentmeter_core::toolcalls::ToolAnalysis;
use runemark::{ColorMode, Console, Tone};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Rows shown per table; the JSON output carries all of them.
const TABLE_ROWS: usize = 20;

/// Colour through runemark's tones, switched off for pipes and `NO_COLOR`. Bars and symbols stay
/// without colour, so a pipe or `NO_COLOR` keeps the picture.
#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
    /// Columns the text fills, from [`MIN_WIDTH`] to [`WIDTH`].
    pub width: usize,
}

impl Style {
    /// A style for `columns` of terminal, or [`WIDTH`] when there is no terminal.
    pub fn new(color: bool, columns: Option<usize>) -> Style {
        Style {
            color,
            width: columns.map_or(WIDTH, |c| c.clamp(MIN_WIDTH, WIDTH)),
        }
    }
    /// Cells of a bar: [`BAR`] at full width, fewer in a narrower terminal.
    fn bar_width(self) -> usize {
        BAR - (WIDTH - self.width).min(BAR - 8)
    }
    fn console(self) -> Console {
        Console::new(
            if self.color {
                ColorMode::Always
            } else {
                ColorMode::Never
            },
            true,
        )
    }
    fn tone(self, tone: Tone, text: &str) -> String {
        self.console().paint(tone, text)
    }
    fn bold(self, text: &str) -> String {
        if self.color {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
    fn dim(self, text: &str) -> String {
        self.tone(Tone::Muted, text)
    }
}

/// What a run selected and measured.
pub struct Report<'a> {
    pub tool: &'a str,
    pub label: &'a str,
    pub period: String,
    pub project: Option<&'a str>,
    /// The directories the logs were read from.
    pub places: &'a [PathBuf],
    pub quality: &'a DataQuality,
    pub summary: Option<&'a Summary>,
    pub sessions: Option<&'a [SessionRow]>,
    pub projects: Option<&'a [ProjectRow]>,
    pub findings: Option<&'a Evaluation>,
    pub probe: Option<&'a Probe>,
    pub instructions: Option<&'a InstructionScan>,
    pub tool_calls: Option<&'a ToolAnalysis>,
    pub transitions: Option<&'a agentmeter_core::transitions::TransitionReport>,
    pub subagents: Option<&'a agentmeter_core::subagents::SubagentReport>,
}

/// Widest the text gets, also in a wider terminal or a pipe; bars are at most [`BAR`] cells.
const WIDTH: usize = 88;
/// Narrower terminals are not followed: the tables need this much.
const MIN_WIDTH: usize = 80;
const BAR: usize = 24;

/// The columns of the terminal on stdout, if stdout is one.
#[cfg(unix)]
pub fn terminal_columns() -> Option<usize> {
    // SAFETY: `winsize` is a plain C struct the ioctl fills completely.
    let size = unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size) != 0 {
            return None;
        }
        size
    };
    (size.ws_col > 0).then_some(usize::from(size.ws_col))
}

#[cfg(not(unix))]
pub fn terminal_columns() -> Option<usize> {
    None
}
/// Width of the label column in breakdowns.
const LABEL: usize = 30;

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
    format!("\n{}\n", style.tone(Tone::Title, title))
}

/// A horizontal bar of `width` cells for `value` of `max`, in eighths; the empty rest is a dim
/// track so bars of one block line up.
fn bar(style: Style, tone: Tone, value: u64, max: u64, width: usize) -> String {
    const EIGHTHS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = if max == 0 {
        0
    } else {
        ((value as f64 / max as f64) * (width * 8) as f64).round() as usize
    };
    // A value above zero always shows.
    let eighths = if value > 0 { eighths.max(1) } else { 0 }.min(width * 8);
    let mut filled = "█".repeat(eighths / 8);
    filled += EIGHTHS[eighths % 8];
    let used = eighths / 8 + usize::from(eighths % 8 > 0);
    format!(
        "{}{}",
        style.tone(tone, &filled),
        style.dim(&"·".repeat(width - used))
    )
}

/// Words wrapped to the style's width: the first line after `first`, the others after `rest`.
/// A word too long for a line of its own, such as a path, is broken after its `/`.
fn wrap(style: Style, text: &str, first: &str, rest: &str, visible_first: usize) -> String {
    let mut out = String::from(first);
    let mut column = visible_first;
    let indent = rest.chars().count();
    let mut at_start = true;
    for word in text.split_whitespace() {
        let pieces: Vec<&str> = if word.chars().count() > style.width - indent {
            word.split_inclusive('/').collect()
        } else {
            vec![word]
        };
        for (i, piece) in pieces.into_iter().enumerate() {
            // Only a word's first piece is set off by a space.
            let space = usize::from(i == 0);
            let len = piece.chars().count();
            if !at_start && column + space + len > style.width {
                out += "\n";
                out += rest;
                column = indent;
                at_start = true;
            }
            if !at_start && space == 1 {
                out.push(' ');
                column += 1;
            }
            out += piece;
            column += len;
            at_start = false;
        }
    }
    out += "\n";
    out
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
    out += &read_text(report, style);
    if let Some(summary) = report.summary {
        out += &summary_text(summary, style);
    }
    if let Some(findings) = report.findings {
        out += &findings_text(findings, report.probe, style);
    }
    if let Some(t) = report.transitions {
        out += &transitions_text(t, style);
    }
    if let Some(s) = report.subagents {
        out += &subagents_text(s, style);
    }
    if let Some(tools) = report.tool_calls {
        out += &tool_calls_text(tools, style);
    }
    if let Some(scan) = report.instructions {
        out += &instructions_text(scan, style);
    }
    if let Some(sessions) = report.sessions {
        out += &sessions_text(sessions, style);
    }
    if let Some(projects) = report.projects {
        out += &projects_text(projects, style);
    }
    out
}

fn duration(ms: i64) -> String {
    let minutes = ms / 60_000;
    match minutes {
        0..60 => format!("{minutes} min"),
        60..2880 => format!("{:.1} h", minutes as f64 / 60.0),
        _ => format!("{:.1} d", minutes as f64 / 1440.0),
    }
}

/// Context size: the log's number, or the word for not recorded.
fn ctx_or_unknown(value: Option<u64>) -> String {
    value.map_or("unknown".to_owned(), tokens)
}

fn transitions_text(r: &agentmeter_core::transitions::TransitionReport, style: Style) -> String {
    let mut out = heading(style, "TRANSITIONS");
    out += &wrap(style, r.method, "  method     ", "             ", 13);
    let c = &r.counts;
    out += &wrap(
        style,
        &format!(
            "{} starts · {} resumed or forked · {} incomplete logs · {} compactions · {} pauses of 1 h or more",
            count(c.starts),
            count(c.resumed_or_forked),
            count(c.incomplete_log),
            count(c.compactions),
            count(c.pauses)
        ),
        "  observed   ",
        "             ",
        13,
    );
    if !r.tool_data {
        out += &wrap(
            style,
            "tool calls were not read: add --tool-calls to check what is read again after a compaction",
            "  note       ",
            "             ",
            13,
        );
    }
    out += "\n";
    for p in r
        .projects
        .iter()
        .filter(|p| p.counts.compactions > 0)
        .take(TABLE_ROWS)
    {
        out += &format!(
            "  {} · {} compactions, median {} before",
            style.bold(&clip(&p.project, 28)),
            count(p.counts.compactions),
            ctx_or_unknown(p.median_context_before_compaction)
        );
        if p.windows_checked > 0 {
            out += &format!(
                " · {} of {} followed by re-read areas ({})",
                count(p.compactions_with_rereads),
                count(p.windows_checked),
                count(p.rereads_after_compaction)
            );
        }
        out += "\n";
        if let Some(h) = &p.hint {
            out += &wrap(style, h, "    hint  ", "          ", 10);
        }
    }
    // The compactions with the most re-read areas, each with its evidence.
    let mut shown: Vec<&agentmeter_core::transitions::Transition> = r
        .transitions
        .iter()
        .filter(|t| t.kind == "compaction")
        .collect();
    shown.sort_by_key(|t| std::cmp::Reverse(t.repeats.as_ref().map_or(0, |x| x.reread_areas)));
    for t in shown.into_iter().take(5) {
        out += &format!("\n  compaction {} · {}\n", t.time, clip(&t.project, 30));
        out += &wrap(
            style,
            &format!(
                "context before {} · after {} (log) · next request {} after {}",
                ctx_or_unknown(t.context_before),
                ctx_or_unknown(t.context_after),
                ctx_or_unknown(t.next_request_context),
                t.ms_to_next_request.map_or("unknown".to_owned(), duration)
            ),
            "    ",
            "    ",
            4,
        );
        out += &wrap(
            style,
            &format!(
                "{} requests until {}",
                count(t.requests_to_next_compaction.unwrap_or(0)),
                if t.then_log_ends == Some(true) {
                    "the end of the log"
                } else {
                    "the next compaction"
                }
            ),
            "    ",
            "    ",
            4,
        );
        if let Some(x) = &t.repeats {
            out += &wrap(
                style,
                &format!(
                    "{} of {} reads in {} cover areas read before; {} repeated failed calls",
                    x.reread_areas, x.reads, x.window, x.failures_repeated
                ),
                "    ",
                "    ",
                4,
            );
            for e in &x.examples {
                out += &wrap(style, e, "    · ", "      ", 6);
            }
        }
        out += &evidence_text(style, &t.file, t.event.as_deref().unwrap_or("-"));
    }
    out += "\n";
    for n in &r.notes {
        out += &wrap(style, n, "  note  ", "        ", 8);
    }
    out
}

fn usage_line(u: &agentmeter_core::subagents::Usage) -> String {
    let mut models: Vec<(&String, &u64)> = u.models.iter().collect();
    models.sort_by_key(|(name, n)| (std::cmp::Reverse(**n), (*name).clone()));
    let mix: Vec<String> = models
        .iter()
        .take(3)
        .map(|(name, n)| format!("{} {}", clip(name, 24), count(**n)))
        .collect();
    format!(
        "{} requests · input {} (fresh {} · cached {} · written {}) · models {}",
        count(u.requests),
        tokens(u.input),
        tokens(u.fresh),
        tokens(u.cache_read),
        tokens(u.cache_write),
        if mix.is_empty() {
            "-".to_owned()
        } else {
            mix.join(", ")
        }
    )
}

fn optional_count(value: Option<u64>) -> String {
    value.map_or("not measurable".to_owned(), count)
}

/// A length of time; zero is "none", since a run of one request has no observed length.
fn run_length(ms: i64) -> String {
    if ms == 0 {
        "none".to_owned()
    } else {
        duration(ms)
    }
}

fn subagents_text(r: &agentmeter_core::subagents::SubagentReport, style: Style) -> String {
    let mut out = heading(style, "SUBAGENTS");
    out += &wrap(style, r.method, "  method     ", "             ", 13);
    let c = &r.counts;
    out += &wrap(
        style,
        &format!(
            "{} sessions · {} delegated · {} agents (deepest chain {}, {} nested) · {} without a named parent",
            count(c.sessions),
            count(c.sessions_with_agents),
            count(c.agents),
            c.max_depth,
            count(c.nested_agents),
            count(c.unassigned_agents)
        ),
        "  observed   ",
        "             ",
        13,
    );
    let t = &r.totals;
    out += &wrap(
        style,
        &format!(
            "of {} requests: {} direct · {} by agents with a named parent · {} by agents without",
            count(t.requests),
            count(t.direct.requests),
            count(t.delegated.requests),
            count(t.unassigned.requests)
        ),
        "  split      ",
        "             ",
        13,
    );
    out += "\n";
    for root in r.roots.iter().take(TABLE_ROWS) {
        out += &format!(
            "  {} · {}{}\n",
            style.bold(&clip(&root.session, 36)),
            clip(&root.project, 28),
            if root.in_logs {
                ""
            } else {
                " · session log not read"
            }
        );
        out += &wrap(
            style,
            &format!("direct   {}", usage_line(&root.direct)),
            "    ",
            "             ",
            4,
        );
        out += &wrap(
            style,
            &format!("agents   {}", usage_line(&root.delegated)),
            "    ",
            "             ",
            4,
        );
        out += &wrap(
            style,
            &format!(
                "{} agents, chain depth {} · input with agents {} (derived, not a total)",
                count(root.agents),
                root.max_depth,
                tokens(root.inclusive_input)
            ),
            "    ",
            "    ",
            4,
        );
        out += &wrap(
            style,
            &format!(
                "{} runs · span {} · inside runs {} · paused {} (run length includes waiting)",
                count(root.runs),
                run_length(root.span_ms),
                run_length(root.active_ms),
                run_length(root.paused_ms)
            ),
            "    ",
            "    ",
            4,
        );
        if let Some(s) = &root.start_context {
            out += &wrap(
                style,
                &format!(
                    "first requests of {} agents sent {} (cached {} · written {}); a measure, not a verdict",
                    count(s.agents),
                    tokens(s.context),
                    tokens(s.cache_read),
                    tokens(s.cache_write)
                ),
                "    ",
                "    ",
                4,
            );
        }
        if r.file_data {
            out += &wrap(
                style,
                &format!(
                    "files: {} areas read by several · {} written by several · {} read at other lines only",
                    optional_count(root.shared_reads),
                    optional_count(root.shared_writes),
                    optional_count(root.shared_reads_other_ranges)
                ),
                "    ",
                "    ",
                4,
            );
            for e in &root.shared_examples {
                out += &wrap(style, e, "    · ", "      ", 6);
            }
        }
        for child in root.children.iter().take(3) {
            out += &wrap(
                style,
                &format!(
                    "agent {} (depth {}, parent {} via {}): {} requests, input {}",
                    child.agent.as_deref().unwrap_or("unknown"),
                    child.depth.map_or("?".to_owned(), |d| d.to_string()),
                    child.parent.as_deref().unwrap_or("unknown"),
                    child.parent_basis.unwrap_or("-"),
                    count(child.usage.requests),
                    tokens(child.usage.input)
                ),
                "    · ",
                "      ",
                6,
            );
        }
        if let Some(first) = root.children.first() {
            out += &evidence_text(style, &first.file, &first.first);
        }
        out += "\n";
    }
    if !r.unassigned.is_empty() {
        out += &format!(
            "  without a named parent ({}):\n",
            count(r.unassigned.len() as u64)
        );
        for child in r.unassigned.iter().take(5) {
            out += &wrap(
                style,
                &format!(
                    "{}: {} requests, input {}",
                    child.agent.as_deref().unwrap_or("unknown agent"),
                    count(child.usage.requests),
                    tokens(child.usage.input)
                ),
                "    · ",
                "      ",
                6,
            );
            out += &evidence_text(style, &child.file, &child.first);
        }
        out += "\n";
    }
    if !r.overlaps.is_empty() {
        out += &format!(
            "  overlapping runs: {} with a delegation, {} agents of one session, {} in time only\n",
            count(c.overlaps_delegation),
            count(c.overlaps_same_task),
            count(c.overlaps_time_only)
        );
        for o in r.overlaps.iter().take(5) {
            let relation = match o.relation {
                "delegation" => "delegation",
                "same_task" => "same session",
                _ => "time only",
            };
            out += &wrap(
                style,
                &format!(
                    "{} + {} · {} · {}{}",
                    clip(&o.first, 28),
                    clip(&o.second, 28),
                    relation,
                    if o.overlap_ms == 0 {
                        "at one request time".to_owned()
                    } else {
                        format!("{} together", duration(o.overlap_ms))
                    },
                    o.shared_writes.map_or(String::new(), |w| format!(
                        " · {} files written by both, {} areas read by both",
                        count(w),
                        count(o.shared_reads.unwrap_or(0))
                    ))
                ),
                "    · ",
                "      ",
                6,
            );
        }
        out += "\n";
    }
    for e in &r.counterexamples {
        out += &wrap(style, e, "  counter  ", "           ", 11);
    }
    for n in &r.notes {
        out += &wrap(style, n, "  note  ", "        ", 8);
    }
    out
}

fn tool_calls_text(a: &ToolAnalysis, style: Style) -> String {
    let mut out = heading(style, "TOOL CALLS");
    out += &wrap(style, a.method, "  method     ", "             ", 13);
    out += &format!(
        "  calls      {} in the period · {} without a result · {} merged duplicates\n",
        style.bold(&count(a.calls)),
        count(a.stats.calls_without_result),
        count(a.stats.duplicates_merged)
    );
    for r in &a.rules {
        let (mark, tone) = match r.status {
            "observed" => ("◆", Tone::Warning),
            "checked_none" => ("✓", Tone::Success),
            _ => ("·", Tone::Muted),
        };
        out += &format!(
            "\n  {} {} (v{}) · {}\n",
            style.tone(tone, mark),
            style.bold(r.id),
            r.version,
            match r.status {
                "observed" => "observed",
                "checked_none" => "checked, nothing to report",
                _ => "not measurable",
            }
        );
        let indent = "    ";
        if let Some(reason) = &r.reason {
            out += &wrap(style, reason, indent, indent, 4);
            continue;
        }
        if let Some(m) = &r.message {
            out += &wrap(style, m, indent, indent, 4);
        }
        if let (Some(from), Some(to)) = (&r.from, &r.to) {
            out += &wrap(
                style,
                &format!("period  {from} to {to}"),
                indent,
                "            ",
                4,
            );
        }
        if let (Some(n), Some(of)) = (r.count, r.of) {
            out += &wrap(
                style,
                &format!(
                    "sample  {} of {} calls · limit: {} (uncalibrated)",
                    count(n),
                    count(of),
                    r.threshold
                ),
                indent,
                "            ",
                4,
            );
        }
        for e in &r.examples {
            let bytes = e.bytes.map_or(String::new(), |b| match b {
                0..1000 => format!(" · {b} B"),
                _ => format!(" · {} KB", b / 1000),
            });
            out += &wrap(
                style,
                &format!("{} × {}{bytes}", count(e.count), e.label),
                "    · ",
                "      ",
                6,
            );
        }
        for c in &r.counterexamples {
            out += &wrap(
                style,
                &format!("not counted  {c}"),
                indent,
                "                 ",
                4,
            );
        }
        if let Some(u) = &r.uncertainty {
            out += &wrap(
                style,
                &format!("uncertain  {u}"),
                indent,
                "               ",
                4,
            );
        }
        for ev in &r.evidence {
            out += &evidence_text(style, &ev.file, &ev.event);
        }
        if let Some(n) = &r.next_step {
            out += &wrap(style, &format!("next  {n}"), indent, "          ", 4);
        }
    }
    out
}

fn state_word(state: LoadState) -> &'static str {
    match state {
        LoadState::Confirmed => "confirmed",
        LoadState::Expected => "expected",
        LoadState::OnDemand => "on demand",
        LoadState::NotLoaded => "not loaded",
        LoadState::Unreadable => "unreadable",
        LoadState::Missing => "missing",
    }
}

fn instructions_text(scan: &InstructionScan, style: Style) -> String {
    let mut out = heading(style, "INSTRUCTIONS");
    out += &wrap(style, &scan.dir, "  directory  ", "             ", 13);
    out += &wrap(
        style,
        scan.rules_version,
        "  rules      ",
        "             ",
        13,
    );
    out += &wrap(style, scan.method, "  method     ", "             ", 13);
    let loaded = scan.files.iter().filter(|f| f.state.is_loaded()).count();
    out += &format!(
        "  at start   {} from {} of {} files (not counted: on demand, not loaded, missing)\n\n",
        style.bold(&format!("≈{}", tokens(scan.loaded_est_tokens))),
        loaded,
        scan.files.len()
    );
    if scan.files.is_empty() {
        out += "  no instruction files found\n";
    }
    // Paths below the scanned directory are shown relative to it; JSON keeps absolute paths.
    let shown = |path: &str| -> String {
        match std::path::Path::new(path).strip_prefix(&scan.dir) {
            Ok(rel) if !rel.as_os_str().is_empty() => format!("./{}", rel.display()),
            _ => path.to_owned(),
        }
    };
    for file in &scan.files {
        let size = if file.bytes == 0 {
            "-".to_owned()
        } else {
            format!("≈{}", tokens(file.est_tokens))
        };
        let prefix = format!("  {:<10} {:>7}  ", state_word(file.state), size);
        let rest = " ".repeat(prefix.chars().count());
        let origin = match &file.via {
            Some(via) => format!("{} (imported by {})", shown(&file.path), shown(via)),
            None => format!("{} [{}]", shown(&file.path), file.scope),
        };
        out += &wrap(style, &origin, &prefix, &rest, prefix.chars().count());
        if let Some(note) = &file.note {
            out += &wrap(style, note, &rest, &rest, rest.chars().count());
        }
    }
    for f in &scan.findings {
        out += &format!(
            "\n  {} {} · certainty {}\n",
            style.tone(Tone::Warning, "◆"),
            style.bold(f.rule),
            f.certainty
        );
        out += &wrap(
            style,
            &format!("file    {}", shown(&f.file)),
            "    ",
            "            ",
            4,
        );
        out += &wrap(
            style,
            &format!("measure {}", f.measure),
            "    ",
            "            ",
            4,
        );
        out += &wrap(
            style,
            &format!("check   {}", f.check),
            "    ",
            "            ",
            4,
        );
    }
    if scan.findings.is_empty() {
        out += "\n  no findings\n";
    }
    out += "\n";
    for note in &scan.notes {
        out += &wrap(style, note, "  note  ", "        ", 8);
    }
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
        style.bold(&count(s.requests)),
        pct(sub)
    );
    out += &format!(
        "  Sessions      {}  in {} projects\n",
        style.bold(&count(s.sessions)),
        count(s.projects)
    );
    out += &format!(
        "  Input         {}  cache read {} · written {} (5 min {} · 1 h {})\n",
        style.bold(&tokens(s.input)),
        pct(s.cache_read_pct),
        tokens(s.cache_write_5m + s.cache_write_1h),
        tokens(s.cache_write_5m),
        tokens(s.cache_write_1h)
    );
    out += &format!(
        "                {} {}\n",
        bar(
            style,
            Tone::Success,
            s.cache_read_pct.round() as u64,
            100,
            style.bar_width()
        ),
        style.dim("share read from cache")
    );
    out += &format!("  Output        {}\n", style.bold(&tokens(s.output)));
    let c = &s.compactions;
    if c.total == 0 {
        out += "  Compactions   none\n";
    } else {
        out += &wrap(
            style,
            &format!(
                "{} (auto {}, manual {}, trigger unknown {}) · before: avg {}, min {}, max {}",
                count(c.total),
                count(c.auto),
                count(c.manual),
                count(c.trigger_unknown),
                tokens(c.avg_tokens_before),
                tokens(c.min_tokens_before),
                tokens(c.max_tokens_before)
            ),
            "  Compactions   ",
            "                ",
            16,
        );
    }
    if !s.days.is_empty() {
        let max = s.days.iter().map(|d| d.input).max().unwrap_or(0);
        out += &heading(style, "DAYS (UTC)");
        out += &style.dim(&format!(
            "  Day          Requests     Input  {:<w$}  Cache  Compactions",
            "Input",
            w = style.bar_width()
        ));
        out += "\n";
        for d in &s.days {
            out += &format!(
                "  {}  {:>8}  {:>8}  {}  {:>5}  {:>11}\n",
                d.date,
                count(d.requests),
                tokens(d.input),
                bar(style, Tone::Info, d.input, max, style.bar_width()),
                pct(d.cache_read_pct),
                d.compactions
            );
        }
    }
    if !s.models.is_empty() {
        let max = s.models.iter().map(|m| m.requests).max().unwrap_or(0);
        out += &heading(style, "MODELS");
        for m in s.models.iter().take(TABLE_ROWS) {
            let name = if m.model.is_empty() {
                "(unknown)"
            } else {
                &m.model
            };
            out += &format!(
                "  {:<28} {:>8}  {}\n",
                clip(name, 28),
                count(m.requests),
                bar(style, Tone::Info, m.requests, max, style.bar_width())
            );
        }
    }
    out
}

/// Symbol, word and tone of a conclusion kind.
fn kind_mark(kind: &str) -> (&'static str, &'static str, Tone) {
    match kind {
        "lever" => ("◆", "lever", Tone::Warning),
        "look" => ("?", "look at it", Tone::Info),
        _ => ("✓", "fine", Tone::Success),
    }
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "lever" => 0,
        "look" => 1,
        _ => 2,
    }
}

fn value_text(value: u64, unit: &str) -> String {
    match unit {
        "tokens" => tokens(value),
        "hours" => format!("{value} h"),
        _ => count(value),
    }
}

/// One row per part: label, bar against the largest part, value and count.
fn parts_text(parts: &[Part], tone: Tone, style: Style) -> String {
    let max = parts.iter().map(|p| p.value).max().unwrap_or(0);
    let mut out = String::new();
    for p in parts {
        let counted = match (p.count, p.count_unit) {
            (Some(n), Some(unit)) => format!("  {} {unit}", count(n)),
            _ => String::new(),
        };
        out += &format!(
            "    {:<LABEL$} {} {:>7}{}\n",
            clip(&p.label, LABEL),
            bar(style, tone, p.value, max, style.bar_width()),
            value_text(p.value, p.unit),
            style.dim(&counted)
        );
    }
    out
}

/// Where the session starts lie: smallest, quartiles, median and largest on one scale.
fn spread_text(f: &Finding, style: Style) -> String {
    let get = |key: &str| f.values.get(key).and_then(Value::as_u64);
    let (Some(min), Some(p25), Some(p75), Some(max)) = (
        get("min_tokens"),
        get("p25_tokens"),
        get("p75_tokens"),
        get("max_tokens"),
    ) else {
        return String::new();
    };
    let median = f.measure.value as u64;
    let width = style.bar_width() + 8;
    let at = |v: u64| ((v as f64 / max.max(1) as f64) * (width - 1) as f64).round() as usize;
    let mut cells = vec![' '; width];
    for (i, cell) in cells.iter_mut().enumerate() {
        if i >= at(min) && i <= at(max) {
            *cell = '─';
        }
        if i >= at(p25) && i <= at(p75) {
            *cell = '━';
        }
    }
    cells[at(min)] = '├';
    cells[at(max)] = '┤';
    cells[at(median)] = '┃';
    let line: String = cells.into_iter().collect();
    format!(
        "    {:<LABEL$} {}\n      {}\n",
        "spread of session starts",
        style.tone(Tone::Info, &line),
        style.dim(&format!(
            "smallest {} · quartiles {} / {} · median {} · largest {}",
            tokens(min),
            tokens(p25),
            tokens(p75),
            tokens(median),
            tokens(max)
        ))
    )
}

/// The median start as one stacked bar: configuration, the rest of the smallest start, and
/// what varies per session.
fn stacked_text(parts: &[Part], style: Style) -> String {
    let total: u64 = parts.iter().map(|p| p.value).sum();
    let tones = [Tone::Warning, Tone::Info, Tone::Muted];
    let glyphs = ["█", "▓", "░"];
    let mut line = String::new();
    let mut used = 0;
    for (i, p) in parts.iter().enumerate().take(3) {
        let cells = if i == parts.len().min(3) - 1 {
            (style.bar_width() + 8).saturating_sub(used)
        } else {
            ((p.value as f64 / total.max(1) as f64) * (style.bar_width() + 8) as f64).round()
                as usize
        };
        used += cells;
        line += &style.tone(tones[i], &glyphs[i].repeat(cells));
    }
    let mut out = format!(
        "    {:<LABEL$} {} {:>7}\n",
        "median start, split",
        line,
        tokens(total)
    );
    for (i, p) in parts.iter().enumerate().take(3) {
        out += &format!(
            "      {} {:<w$} {:>7}\n",
            style.tone(tones[i], glyphs[i]),
            p.label,
            tokens(p.value),
            w = LABEL - 3 + style.bar_width() + 8
        );
    }
    out
}

/// An evidence line: file and event, broken before ` · ` and after `/` where the line would
/// be too wide, the rest under the first.
fn evidence_text(style: Style, file: &str, event: &str) -> String {
    const INDENT: &str = "              ";
    let room = style.width - INDENT.len();
    let mut pieces: Vec<String> = file.split_inclusive('/').map(str::to_owned).collect();
    pieces.push(format!(" · {event}"));
    let mut lines: Vec<String> = vec![String::new()];
    for piece in pieces {
        let line = lines.last_mut().expect("lines starts with one");
        if line.is_empty() || line.chars().count() + piece.chars().count() <= room {
            *line += &piece;
        } else {
            lines.push(piece.trim_start().to_owned());
        }
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out += if i == 0 { "    evidence  " } else { INDENT };
        out += line;
        out += "\n";
    }
    out
}

/// What `/context` reported: the configuration run in full, the project runs as differences.
fn probe_text(probe: &Probe, style: Style) -> String {
    let mut out = String::new();
    if let Some(base) = probe.baseline() {
        out += &format!(
            "\n    {}\n",
            style.bold(&format!(
                "today's configuration, /context in an empty directory{}",
                base.model
                    .as_deref()
                    .map_or(String::new(), |m| format!(" · {m}"))
            ))
        );
        let max = base.sent.iter().map(|i| i.tokens).max().unwrap_or(0);
        for item in &base.sent {
            out += &format!(
                "    {:<LABEL$} {} {:>7}\n",
                clip(&item.label, LABEL),
                bar(style, Tone::Warning, item.tokens, max, style.bar_width()),
                tokens(item.tokens)
            );
        }
        let list = |items: &[ProbeItem], n: usize| {
            items
                .iter()
                .take(n)
                .map(|i| format!("{} {}", i.label, tokens(i.tokens)))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        if !base.deferred.is_empty() {
            out += &wrap(
                style,
                &style.dim(&format!(
                    "loaded on demand, not sent at start: {}",
                    list(&base.deferred, 5)
                )),
                "    ",
                "    ",
                4,
            );
        }
        if !base.mcp_servers.is_empty() {
            out += &wrap(
                style,
                &format!("MCP servers: {}", list(&base.mcp_servers, 6)),
                "    ",
                "      ",
                4,
            );
        }
        if !base.memory_files.is_empty() {
            out += &wrap(
                style,
                &format!("memory files: {}", list(&base.memory_files, 6)),
                "    ",
                "      ",
                4,
            );
        }
        let projects: Vec<String> = probe
            .runs
            .iter()
            .filter(|r| r.place != PROBE_EMPTY_PLACE)
            .map(|r| {
                let delta = r.total as i64 - base.total as i64;
                let sign = if delta >= 0 { "+" } else { "−" };
                format!("{} {sign}{}", r.place, tokens(delta.unsigned_abs()))
            })
            .collect();
        if !projects.is_empty() {
            out += &wrap(
                style,
                &format!("in the project directory: {}", projects.join(" · ")),
                "    ",
                "      ",
                4,
            );
        }
    }
    for failure in &probe.failures {
        out += &format!(
            "    {} probe in {} failed: {}\n",
            style.tone(Tone::Error, "✗"),
            failure.place,
            failure.reason
        );
    }
    out
}

fn findings_text(evaluation: &Evaluation, probe: Option<&Probe>, style: Style) -> String {
    let mut out = String::new();
    let tally = |kind: &str| {
        evaluation
            .findings
            .iter()
            .filter(|f| f.conclusion.kind == kind)
            .count()
    };
    let status = |s: &str| evaluation.rules.iter().filter(|r| r.status == s).count();
    let headline = [
        (tally("lever"), "lever", "levers", "◆", Tone::Warning),
        (tally("look"), "to look at", "to look at", "?", Tone::Info),
        (tally("fine"), "fine", "fine", "✓", Tone::Success),
        (
            status("checked_none"),
            "nothing found",
            "nothing found",
            "✓",
            Tone::Success,
        ),
        (
            status("not_measurable"),
            "not measurable",
            "not measurable",
            "·",
            Tone::Muted,
        ),
    ]
    .iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many, mark, tone)| {
        style.tone(
            *tone,
            &format!("{mark} {n} {}", if *n == 1 { one } else { many }),
        )
    })
    .collect::<Vec<_>>()
    .join("   ");
    out += &format!("\n{}   {headline}\n", style.tone(Tone::Title, "FINDINGS"));
    out += &style.dim(&format!("{}\n", "─".repeat(style.width)));
    match evaluation.verdict {
        "no_data" => {
            out += "  No analysis possible: no requests with usage in the selected period.\n"
        }
        "none" => {
            out += "  No findings. Every rule that could be measured found nothing to report.\n"
        }
        _ => {}
    }
    let mut findings: Vec<&Finding> = evaluation.findings.iter().collect();
    findings.sort_by_key(|f| (kind_rank(f.conclusion.kind), f.level));
    for f in findings {
        let (mark, word, tone) = kind_mark(f.conclusion.kind);
        let title = format!("{mark} {}", f.rule);
        let tags = format!(
            "{word} · {} · {} · v{}",
            f.level.label(),
            f.certainty,
            f.rule_version
        );
        let pad = style
            .width
            .saturating_sub(title.chars().count() + tags.chars().count() + 2);
        out += &format!(
            "\n{}{}{}\n",
            style.tone(tone, &title),
            " ".repeat(pad + 2),
            style.dim(&tags)
        );
        out += &wrap(style, &style.bold(&f.conclusion.text), "  ⇒ ", "    ", 4);
        out += "\n";
        if f.rule == "baseline-load" {
            out += &spread_text(f, style);
            if probe.and_then(Probe::baseline).is_some() {
                out += &stacked_text(&f.parts, style);
            } else {
                out += &parts_text(&f.parts, tone, style);
            }
            if let Some(probe) = probe {
                out += &probe_text(probe, style);
            }
        } else {
            out += &parts_text(&f.parts, tone, style);
        }
        if f.rule == "cache-rebuild" {
            let get = |key: &str| f.values.get(key).and_then(Value::as_u64);
            if let (Some(saved), Some(extra)) = (
                get("ttl_1h_saved_equivalent"),
                get("ttl_1h_extra_equivalent"),
            ) {
                out += &format!(
                    "    {:<LABEL$} {}\n",
                    "1 h lifetime, estimate",
                    style.dim(&format!(
                        "saves {} · costs {} more · net {}{}",
                        tokens(saved),
                        tokens(extra),
                        if saved >= extra { "+" } else { "−" },
                        tokens(saved.abs_diff(extra))
                    ))
                );
            }
        }
        out += "\n";
        out += &wrap(style, &f.message, "    measured  ", "              ", 14);
        let m = &f.measure;
        out += &wrap(
            style,
            &style.dim(&format!(
                "{} = {} {} of {} {} (sample {} {})",
                m.name,
                m.value,
                m.unit,
                count(m.denominator),
                m.denominator_unit,
                count(m.sample),
                m.sample_unit
            )),
            "              ",
            "              ",
            14,
        );
        for e in &f.evidence {
            out += &style.dim(&evidence_text(style, &e.file, &e.event));
        }
        for c in &f.counterexamples {
            out += &wrap(
                style,
                &style.dim(c),
                &style.dim("    but       "),
                "              ",
                14,
            );
        }
        out += &wrap(
            style,
            &f.next_step,
            &format!("    {}      ", style.bold("next")),
            "              ",
            14,
        );
    }
    // Every rule appears once per source: observed above, the others as checked or not measurable.
    let quiet: Vec<_> = evaluation
        .rules
        .iter()
        .filter(|r| r.status != "observed")
        .collect();
    if !quiet.is_empty() {
        out += "\n";
    }
    for r in quiet.iter() {
        let (mark, tone, line) = match (r.status, &r.message, &r.reason) {
            ("checked_none", Some(message), _) => {
                ("✓", Tone::Success, format!("nothing to report: {message}"))
            }
            (_, _, reason) => (
                "·",
                Tone::Muted,
                format!("not measurable: {}", reason.as_deref().unwrap_or("")),
            ),
        };
        let first = format!("{} {:<16}", style.tone(tone, mark), r.rule);
        out += &wrap(
            style,
            &style.dim(&format!("{line} (v{})", r.rule_version)),
            &first,
            "                  ",
            18,
        );
    }
    // Without a baseline-load card the probe still has its say.
    if let Some(probe) = probe.filter(|_| {
        !evaluation
            .findings
            .iter()
            .any(|f| f.rule == "baseline-load")
    }) {
        out += &format!("\n{}\n", style.tone(Tone::Title, "CONTEXT PROBE"));
        out += &probe_text(probe, style);
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
    let max = rows.iter().map(|r| r.input).max().unwrap_or(0);
    // The columns without the bar take 78; the bar gets what is left, or goes.
    let bar_cells = table_bar(style, 78);
    out += &style.dim(&format!(
        "  Session   Project        Requests   Span     Peak   Input  {}Cache  Compact",
        bar_cells.map_or(String::new(), |w| format!("{:<w$}  ", ""))
    ));
    out += "\n";
    for r in rows.iter().take(TABLE_ROWS) {
        out += &format!(
            "  {:<8}  {:<14} {:>8}  {:>5.1} h  {:>6}  {:>6}  {}{:>5}  {:>7}\n",
            clip(&r.session, 8),
            clip(&r.project, 14),
            count(r.requests),
            r.hours,
            tokens(r.peak_context),
            tokens(r.input),
            bar_cells.map_or(String::new(), |w| format!(
                "{}  ",
                bar(style, Tone::Info, r.input, max, w)
            )),
            pct(r.cache_read_pct),
            r.compactions
        );
    }
    out
}

/// Cells for a table's bar column next to `columns` of other columns: at most [`BAR`], none
/// if fewer than 6 fit.
fn table_bar(style: Style, columns: usize) -> Option<usize> {
    let cells = style.width.saturating_sub(columns).min(BAR);
    (cells >= 6).then_some(cells)
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
    let bar_cells = table_bar(style, 69);
    out += &style.dim(&format!(
        "  Project                  Sessions  Requests   Input  Share  {}Cache",
        bar_cells.map_or(String::new(), |w| format!("{:<w$}  ", ""))
    ));
    out += "\n";
    for r in rows.iter().take(TABLE_ROWS) {
        out += &format!(
            "  {:<24} {:>8}  {:>8}  {:>6}  {:>5}  {}{:>5}\n",
            clip(&r.project, 24),
            count(r.sessions),
            count(r.requests),
            tokens(r.input),
            pct(r.input_share_pct),
            bar_cells.map_or(String::new(), |w| format!(
                "{}  ",
                bar(
                    style,
                    Tone::Info,
                    (r.input_share_pct * 10.0).round() as u64,
                    1000,
                    w
                )
            )),
            pct(r.cache_read_pct)
        );
    }
    out
}

/// What was read and how well: one line per stage, a mark for its state.
fn read_text(report: &Report, style: Style) -> String {
    let quality = report.quality;
    let stats: &ReadStats = &quality.read;
    let ok = style.tone(Tone::Success, "✓");
    let note = style.tone(Tone::Warning, "!");
    let bad = style.tone(Tone::Error, "✗");
    let mut out = heading(style, "READ");
    let line = |mark: &str, what: &str, text: String| {
        wrap(
            style,
            &text,
            &format!("  {mark} {what:<9} "),
            "              ",
            14,
        )
    };
    let places: Vec<String> = report
        .places
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let versions = match (&stats.first_version, &stats.last_version) {
        (Some(a), Some(b)) if a != b => format!(" · version {a} – {b}"),
        (Some(a), _) => format!(" · version {a}"),
        _ => String::new(),
    };
    out += &line(
        if stats.files > 0 { &ok } else { &bad },
        "logs",
        format!(
            "{} files, {} lines from {}{versions}",
            count(stats.files),
            count(stats.lines),
            if places.is_empty() {
                "(none)".to_owned()
            } else {
                places.join(", ")
            }
        ),
    );
    match (&quality.observed_from, &quality.observed_to) {
        (Some(from), Some(to)) => {
            out += &line(
                &ok,
                "observed",
                format!(
                    "{from} – {to} · {} requests in all",
                    count(quality.requests)
                ),
            )
        }
        _ => out += &line(&bad, "observed", "no requests with usage".to_owned()),
    }

    // Problems lose data; limits leave something unmeasured; info only says what was done.
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    let mut info = Vec::new();
    if stats.skipped_lines > 0 {
        problems.push(format!(
            "{} unreadable entries skipped",
            count(stats.skipped_lines)
        ));
    }
    if stats.compressed_files_unreadable > 0 {
        problems.push(format!(
            "{} compressed files unreadable (damaged or over the limits)",
            count(stats.compressed_files_unreadable)
        ));
    }
    if stats.compressed_files_read
        + stats.compressed_files_unreadable
        + stats.compressed_files_shadowed
        > 0
    {
        info.push(format!(
            "compressed files (.zst): {} read, {} unreadable (damaged or over the limits), {} skipped because a plain copy exists",
            count(stats.compressed_files_read),
            count(stats.compressed_files_unreadable),
            count(stats.compressed_files_shadowed)
        ));
    }
    if stats.files_without_usage > 0 {
        notes.push(format!(
            "{} files without usage",
            count(stats.files_without_usage)
        ));
    }
    if stats.placeholder_version_entries > 0 {
        info.push(format!(
            "{} entries carry the placeholder version 0.0.0, left out of the version range",
            count(stats.placeholder_version_entries)
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
        info.push(format!(
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
    if problems.is_empty() {
        out += &line(&ok, "quality", "every line readable".to_owned());
    }
    for problem in problems {
        out += &line(&bad, "quality", problem);
    }
    for n in notes {
        out += &line(&note, "limit", n);
    }
    let dot = style.dim("·");
    for n in info {
        out += &line(&dot, "info", n);
    }
    if let Some(evaluation) = report.findings {
        let measured = evaluation
            .rules
            .iter()
            .filter(|r| r.status != "not_measurable")
            .count();
        let total = evaluation.rules.len();
        out += &line(
            if measured == total { &ok } else { &note },
            "rules",
            format!("{measured} of {total} measurable in this source"),
        );
    }
    if let Some(probe) = report.probe {
        let mark = if probe.failures.is_empty() {
            &ok
        } else if probe.runs.is_empty() {
            &bad
        } else {
            &note
        };
        let places: Vec<&str> = probe.runs.iter().map(|r| r.place.as_str()).collect();
        out += &line(
            mark,
            "probe",
            format!(
                "/context in {} of {} places ({}); answered locally, no model request · {}",
                probe.runs.len(),
                probe.runs.len() + probe.failures.len(),
                if places.is_empty() {
                    "none".to_owned()
                } else {
                    places.join(", ")
                },
                probe.command
            ),
        );
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
    if let Some(v) = report.probe {
        put("probe", json!(v));
    }
    if let Some(v) = report.instructions {
        put("instructions", json!(v));
    }
    if let Some(v) = report.tool_calls {
        put("tool_calls", json!(v));
    }
    if let Some(v) = report.transitions {
        put("transitions", json!(v));
    }
    if let Some(v) = report.subagents {
        put("subagents", json!(v));
    }
    if let Some(v) = report.findings {
        put("verdict", json!(v.verdict));
        put("findings", json!(v.findings));
        put("rules", json!(v.rules));
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str =
        "-Users-me-GitHub-ai-agent-config/76c6c7ec-4dbe-4027-b596-a6e9912d95e2.jsonl";

    #[test]
    fn evidence_breaks_after_a_slash_and_before_the_event() {
        let style = Style::new(false, None);
        assert_eq!(
            evidence_text(style, FILE, "msg_01S8BuvoxgQfkPcmJczZ5xH1"),
            "    evidence  -Users-me-GitHub-ai-agent-config/\n\
             \x20             76c6c7ec-4dbe-4027-b596-a6e9912d95e2.jsonl · msg_01S8BuvoxgQfkPcmJczZ5xH1\n"
        );
        assert_eq!(
            evidence_text(style, "a/b.jsonl", "line:3"),
            "    evidence  a/b.jsonl · line:3\n"
        );
    }

    #[test]
    fn a_narrow_terminal_shrinks_lines_and_a_wide_one_stays_at_88() {
        assert_eq!(Style::new(false, Some(200)).width, WIDTH);
        assert_eq!(Style::new(false, Some(40)).width, MIN_WIDTH);
        for columns in [80, 84, 88] {
            let style = Style::new(false, Some(columns));
            let text = evidence_text(style, FILE, "msg_01S8BuvoxgQfkPcmJczZ5xH1")
                + &wrap(style, &"word ".repeat(40), "  ", "  ", 2)
                + &wrap(
                    style,
                    &format!("from /{}", "directory/".repeat(12)),
                    "  ",
                    "    ",
                    2,
                )
                + &bar(style, Tone::Info, 1, 2, style.bar_width());
            for line in text.lines() {
                assert!(line.chars().count() <= columns, "{columns}: {line}");
            }
        }
    }
}
