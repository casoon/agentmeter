//! `--tool-calls`: repeated reads, repeated failures and large tool outputs.
//!
//! The adapters read a minimal [`ToolEvent`] per call, only when asked for. A call and its result
//! are joined by their id; commands and other parameters are kept as a fingerprint (a hash), the
//! output only as a byte count. Neither text is stored or printed. Only file paths of file tools
//! are kept, to name the file that was read again.
//!
//! Output volume is bytes (tokens are never attributed to one tool: a request's tokens are
//! shared by everything in its context). The rule limits are uncalibrated.

use crate::model::Compaction;
use crate::time::format_iso;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// What a call does, as far as the adapter can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Reads a file (target and range known).
    Read,
    /// Edits or writes a file.
    Write,
    /// Runs a command; only its fingerprint is kept.
    Command,
    Other,
}

/// How a call ended; unknown is not success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Ok,
    Error,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEvent {
    /// Index of the log file; one file is one context stream.
    pub source: usize,
    pub session: String,
    pub project: String,
    pub sub: bool,
    /// Line order inside the file.
    pub seq: u64,
    pub time_ms: i64,
    /// Id of the call; evidence reference together with the file.
    pub call_id: String,
    pub tool: String,
    pub kind: ToolKind,
    /// File path of a read or write; `None` otherwise.
    pub target: Option<String>,
    /// Lines `(start, end)` of a partial read; `None` is the whole file.
    pub range: Option<(u64, u64)>,
    /// Hash of the parameters that make two calls identical; `None` when not known.
    pub fingerprint: Option<u64>,
    pub status: ToolStatus,
    /// Bytes of the text of the result; `None` without a result or when not measurable.
    pub output_bytes: Option<u64>,
    pub has_result: bool,
}

/// What reading the tool events saw.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ToolStats {
    pub calls: u64,
    pub calls_without_result: u64,
    pub results_without_call: u64,
    /// Streaming copies and replays of a call that were merged into one.
    pub duplicates_merged: u64,
    /// Calls per tool name.
    pub tools: BTreeMap<String, u64>,
}

#[derive(Debug, Default, Clone)]
pub struct ToolData {
    pub events: Vec<ToolEvent>,
    pub stats: ToolStats,
    /// The log names the file or command a call works on.
    pub has_targets: bool,
    /// The log says whether a call failed.
    pub has_status: bool,
    /// The log carries the size of results.
    pub has_sizes: bool,
}

impl ToolData {
    /// Keeps only projects whose name contains `name`, ignoring case.
    pub fn retain_project(&mut self, name: &str) {
        let needle = name.to_lowercase();
        self.events
            .retain(|e| e.project.to_lowercase().contains(&needle));
    }
}

/// A fingerprint short enough to read: eight hex digits.
pub fn short_id(fingerprint: u64) -> String {
    format!("{:08x}", fingerprint >> 32)
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolExample {
    /// A file path, or a tool name with a parameter fingerprint (`Bash #a1b2c3d4`).
    pub label: String,
    pub count: u64,
    /// Output bytes of these calls, where known.
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolEvidence {
    pub file: String,
    pub event: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolRule {
    pub id: &'static str,
    pub version: u32,
    /// `observed`, `checked_none` or `not_measurable`.
    pub status: &'static str,
    pub reason: Option<String>,
    pub message: Option<String>,
    /// Calls that match the rule.
    pub count: Option<u64>,
    /// Calls the rule looked at (the denominator).
    pub of: Option<u64>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub threshold: &'static str,
    pub calibrated: bool,
    pub examples: Vec<ToolExample>,
    /// Cases that look like the pattern but are not counted, with their number.
    pub counterexamples: Vec<String>,
    pub uncertainty: Option<String>,
    pub evidence: Vec<ToolEvidence>,
    pub next_step: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolAnalysis {
    pub method: &'static str,
    /// Calls in the selected period.
    pub calls: u64,
    pub stats: ToolStats,
    pub rules: Vec<ToolRule>,
}

const METHOD: &str = "tool calls joined to their results by id; commands as fingerprints, outputs \
                      as byte counts; no command or output text is kept";

/// Reads of the same file area from this count on are reported.
const REPEAT_READS: u64 = 5;
/// Failed calls repeated unchanged from this count on are reported.
const REPEAT_FAILURES: u64 = 3;
/// A single result of this many bytes (about 25k tokens at 4 bytes per token) is "large".
const LARGE_BYTES: u64 = 100_000;
const MAX_EXAMPLES: usize = 5;

fn rule(id: &'static str, threshold: &'static str) -> ToolRule {
    ToolRule {
        id,
        version: 1,
        status: "not_measurable",
        reason: None,
        message: None,
        count: None,
        of: None,
        from: None,
        to: None,
        threshold,
        calibrated: false,
        examples: Vec::new(),
        counterexamples: Vec::new(),
        uncertainty: None,
        evidence: Vec::new(),
        next_step: None,
    }
}

fn not_measurable(mut r: ToolRule, reason: &str) -> ToolRule {
    r.reason = Some(reason.to_owned());
    r
}

/// Evaluates the three rules over the events in the period. `files` names the log files by
/// `source`; `compactions` excuse a second read.
pub fn evaluate(
    data: &ToolData,
    files: &[String],
    compactions: &[Compaction],
    since_ms: Option<i64>,
) -> ToolAnalysis {
    let events: Vec<&ToolEvent> = data
        .events
        .iter()
        .filter(|e| since_ms.is_none_or(|s| e.time_ms >= s))
        .collect();
    let mut stats = data.stats.clone();
    stats.calls = events.len() as u64;
    stats.calls_without_result = events.iter().filter(|e| !e.has_result).count() as u64;
    stats.tools.clear();
    for e in &events {
        *stats.tools.entry(e.tool.clone()).or_default() += 1;
    }
    let mut writes: HashMap<&str, Vec<&ToolEvent>> = HashMap::new();
    for e in events.iter().filter(|e| e.kind == ToolKind::Write) {
        writes.entry(e.session.as_str()).or_default().push(e);
    }
    let ctx = Ctx {
        events: &events,
        writes,
        files,
        compactions,
        from: events.iter().map(|e| e.time_ms).min().map(format_iso),
        to: events.iter().map(|e| e.time_ms).max().map(format_iso),
    };
    let rules = vec![
        repeat_read(&ctx, data),
        repeat_failure(&ctx, data),
        large_output(&ctx, data),
    ];
    ToolAnalysis {
        method: METHOD,
        calls: events.len() as u64,
        stats,
        rules,
    }
}

struct Ctx<'a> {
    events: &'a [&'a ToolEvent],
    files: &'a [String],
    compactions: &'a [Compaction],
    /// Observed writes by session, for the "edit in between" checks.
    writes: HashMap<&'a str, Vec<&'a ToolEvent>>,
    from: Option<String>,
    to: Option<String>,
}

impl Ctx<'_> {
    fn evidence(&self, e: &ToolEvent) -> ToolEvidence {
        ToolEvidence {
            file: self.files.get(e.source).cloned().unwrap_or_default(),
            event: e.call_id.clone(),
        }
    }

    fn base(&self, id: &'static str, threshold: &'static str) -> ToolRule {
        let mut r = rule(id, threshold);
        r.from = self.from.clone();
        r.to = self.to.clone();
        r
    }

    fn compacted_between(&self, session: &str, from_ms: i64, to_ms: i64) -> bool {
        self.compactions
            .iter()
            .any(|c| c.session == session && c.time_ms > from_ms && c.time_ms < to_ms)
    }

    /// An observed write in the session in `(from, to)`: by the file when given, any file else.
    fn written_between(
        &self,
        session: &str,
        target: Option<&str>,
        a: &ToolEvent,
        b: &ToolEvent,
    ) -> bool {
        self.writes.get(session).is_some_and(|writes| {
            writes.iter().any(|w| {
                (w.time_ms, w.seq) > (a.time_ms, a.seq_if_same(w))
                    && (w.time_ms, w.seq) < (b.time_ms, b.seq_if_same(w))
                    && target.is_none_or(|t| w.target.as_deref() == Some(t))
            })
        })
    }
}

impl ToolEvent {
    /// Sequence numbers only order lines of one file; across files the time decides.
    fn seq_if_same(&self, other: &ToolEvent) -> u64 {
        if self.source == other.source {
            self.seq
        } else if other.time_ms > self.time_ms {
            u64::MAX
        } else {
            0
        }
    }
}

fn label(e: &ToolEvent) -> String {
    match (&e.target, e.fingerprint) {
        (Some(t), _) => t.clone(),
        (None, Some(f)) => format!("{} #{}", e.tool, short_id(f)),
        (None, None) => e.tool.clone(),
    }
}

fn top_examples(map: HashMap<String, (u64, Option<u64>)>) -> Vec<ToolExample> {
    let mut list: Vec<_> = map.into_iter().collect();
    list.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    list.into_iter()
        .take(MAX_EXAMPLES)
        .map(|(label, (count, bytes))| ToolExample {
            label,
            count,
            bytes,
        })
        .collect()
}

fn sorted(mut v: Vec<&ToolEvent>) -> Vec<&ToolEvent> {
    v.sort_by_key(|e| (e.time_ms, e.seq));
    v
}

/// Source file, path and line range: what makes two reads the same area.
type ReadKey<'a> = (usize, &'a str, Option<(u64, u64)>);

fn repeat_read(ctx: &Ctx, data: &ToolData) -> ToolRule {
    let r = ctx.base("repeat-read", "5 re-reads of the same file area");
    if !data.has_targets {
        return not_measurable(r, "the log does not name the file a call reads");
    }
    let reads: Vec<&ToolEvent> = ctx
        .events
        .iter()
        .copied()
        .filter(|e| e.kind == ToolKind::Read && e.target.is_some())
        .collect();
    if reads.is_empty() {
        return not_measurable(r, "no file reads in the period");
    }
    let mut groups: HashMap<ReadKey, Vec<&ToolEvent>> = HashMap::new();
    for e in &reads {
        groups
            .entry((e.source, e.target.as_deref().unwrap_or(""), e.range))
            .or_default()
            .push(e);
    }
    let mut repeats = 0u64;
    let mut excused_write = 0u64;
    let mut excused_compaction = 0u64;
    let mut examples: HashMap<String, (u64, Option<u64>)> = HashMap::new();
    let mut evidence = Vec::new();
    for group in groups.into_values() {
        let group = sorted(group);
        for pair in group.windows(2) {
            let (prev, cur) = (pair[0], pair[1]);
            if ctx.written_between(&cur.session, cur.target.as_deref(), prev, cur) {
                excused_write += 1;
            } else if ctx.compacted_between(&cur.session, prev.time_ms, cur.time_ms) {
                excused_compaction += 1;
            } else {
                repeats += 1;
                let entry = examples.entry(label(cur)).or_default();
                entry.0 += 1;
                if let Some(b) = cur.output_bytes {
                    *entry.1.get_or_insert(0) += b;
                }
                if evidence.len() < MAX_EXAMPLES {
                    evidence.push(ctx.evidence(cur));
                }
            }
        }
    }
    let mut r = r;
    r.of = Some(reads.len() as u64);
    r.count = Some(repeats);
    r.counterexamples = vec![
        format!("{excused_write} re-reads after an observed edit of that file: not counted"),
        format!("{excused_compaction} re-reads after a context compaction: not counted"),
        "other line ranges of the same file are separate areas".to_owned(),
    ];
    if repeats >= REPEAT_READS {
        r.status = "observed";
        r.message = Some(format!(
            "{repeats} of {} file reads repeat an earlier read of the same area with no observed \
             edit or compaction in between",
            reads.len()
        ));
        r.examples = top_examples(examples);
        r.evidence = evidence;
        r.uncertainty = Some(
            "Edits made by commands (shell, git, a formatter) are not observed, so a re-read \
             can still have been justified; the file may also have changed outside the session."
                .to_owned(),
        );
        r.next_step = Some(
            "Open the evidence sessions and check why the file was read again; read the \
             area you need (offset and limit) or keep the file's facts in a note."
                .to_owned(),
        );
    } else {
        r.status = "checked_none";
        r.message = Some(format!(
            "{repeats} unexplained re-reads in {} reads, below {REPEAT_READS}",
            reads.len()
        ));
    }
    r
}

fn repeat_failure(ctx: &Ctx, data: &ToolData) -> ToolRule {
    let r = ctx.base("repeat-failure", "3 identical failed calls");
    if !data.has_targets || !data.has_status {
        return not_measurable(
            r,
            "the log neither names what a call runs nor says whether it failed",
        );
    }
    let calls: Vec<&ToolEvent> = ctx
        .events
        .iter()
        .copied()
        .filter(|e| e.fingerprint.is_some() && e.kind != ToolKind::Write)
        .collect();
    let known: Vec<&&ToolEvent> = calls
        .iter()
        .filter(|e| e.status != ToolStatus::Unknown)
        .collect();
    if known.is_empty() {
        return not_measurable(r, "no call with a known result in the period");
    }
    let mut groups: HashMap<(usize, &str, u64), Vec<&ToolEvent>> = HashMap::new();
    for e in &calls {
        groups
            .entry((e.source, e.tool.as_str(), e.fingerprint.unwrap_or(0)))
            .or_default()
            .push(e);
    }
    let mut repeats = 0u64;
    let mut excused_write = 0u64;
    let mut examples: HashMap<String, (u64, Option<u64>)> = HashMap::new();
    let mut evidence = Vec::new();
    for group in groups.into_values() {
        let group = sorted(group);
        for pair in group.windows(2) {
            let (prev, cur) = (pair[0], pair[1]);
            if prev.status != ToolStatus::Error {
                continue;
            }
            if ctx.written_between(&cur.session, None, prev, cur) {
                excused_write += 1;
            } else {
                repeats += 1;
                let entry = examples.entry(label(cur)).or_default();
                entry.0 += 1;
                if let Some(b) = cur.output_bytes {
                    *entry.1.get_or_insert(0) += b;
                }
                if evidence.len() < MAX_EXAMPLES {
                    evidence.push(ctx.evidence(cur));
                }
            }
        }
    }
    let mut r = r;
    r.of = Some(known.len() as u64);
    r.count = Some(repeats);
    r.counterexamples = vec![format!(
        "{excused_write} repeats after an observed file edit in the session (a retest after a fix): not counted"
    )];
    if repeats >= REPEAT_FAILURES {
        r.status = "observed";
        r.message = Some(format!(
            "{repeats} calls repeat an identical failed call with no observed edit in between"
        ));
        r.examples = top_examples(examples);
        r.evidence = evidence;
        r.uncertainty = Some(
            "Changes made by commands are not observed, and a failure caused by something \
             outside the session (network, a service) can be worth retrying. Commands are shown \
             as fingerprints only."
                .to_owned(),
        );
        r.next_step = Some(
            "Open the evidence and check what changed between the attempts; if nothing, \
             fix the cause before the next try or tell the agent to stop retrying."
                .to_owned(),
        );
    } else {
        r.status = "checked_none";
        r.message = Some(format!(
            "{repeats} unchanged repeats of failed calls, below {REPEAT_FAILURES}"
        ));
    }
    r
}

fn large_output(ctx: &Ctx, data: &ToolData) -> ToolRule {
    let r = ctx.base("large-output", "a result of 100 KB or more");
    if !data.has_sizes {
        return not_measurable(r, "the log does not carry the size of tool results");
    }
    let sized: Vec<&ToolEvent> = ctx
        .events
        .iter()
        .copied()
        .filter(|e| e.output_bytes.is_some())
        .collect();
    if sized.is_empty() {
        return not_measurable(r, "no tool result with a size in the period");
    }
    let total: u64 = sized.iter().filter_map(|e| e.output_bytes).sum();
    let large: Vec<&ToolEvent> = sized
        .iter()
        .copied()
        .filter(|e| e.output_bytes.unwrap_or(0) >= LARGE_BYTES)
        .collect();
    let large_bytes: u64 = large.iter().filter_map(|e| e.output_bytes).sum();
    let mut r = r;
    r.of = Some(sized.len() as u64);
    r.count = Some(large.len() as u64);
    if large.is_empty() {
        r.status = "checked_none";
        r.message = Some(format!(
            "no result of {} KB or more in {} results ({} KB in total)",
            LARGE_BYTES / 1000,
            sized.len(),
            total / 1000
        ));
        return r;
    }
    let mut by_tool: HashMap<String, (u64, Option<u64>)> = HashMap::new();
    for e in &large {
        let entry = by_tool.entry(label_tool(e)).or_default();
        entry.0 += 1;
        *entry.1.get_or_insert(0) += e.output_bytes.unwrap_or(0);
    }
    let mut biggest = large.clone();
    biggest.sort_by_key(|e| std::cmp::Reverse(e.output_bytes));
    r.status = "observed";
    r.message = Some(format!(
        "{} of {} results are {} KB or more: {} KB of {} KB returned in total ({:.0} %)",
        large.len(),
        sized.len(),
        LARGE_BYTES / 1000,
        large_bytes / 1000,
        total / 1000,
        large_bytes as f64 * 100.0 / total.max(1) as f64
    ));
    r.examples = top_examples(by_tool);
    r.evidence = biggest
        .iter()
        .take(MAX_EXAMPLES)
        .map(|e| ctx.evidence(e))
        .collect();
    r.counterexamples = vec![
        "Bytes are the size of the returned text; images are not counted. A large result is \
         not wasted when the agent needed all of it."
            .to_owned(),
    ];
    r.uncertainty = Some(
        "Bytes, not tokens: the tokens of a request are not attributed to one tool. About \
         4 bytes per token is a rule of thumb for text."
            .to_owned(),
    );
    r.next_step = Some(
        "Check what the largest calls returned (open the evidence): narrow a search, read a \
         line range, filter or truncate build and test output before it reaches the context."
            .to_owned(),
    );
    r
}

fn label_tool(e: &ToolEvent) -> String {
    e.tool.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn event(
        seq: u64,
        time_ms: i64,
        tool: &str,
        kind: ToolKind,
        target: Option<&str>,
    ) -> ToolEvent {
        ToolEvent {
            source: 0,
            session: "s".into(),
            project: "p".into(),
            sub: false,
            seq,
            time_ms,
            call_id: format!("id{seq}"),
            tool: tool.into(),
            kind,
            target: target.map(str::to_owned),
            range: None,
            fingerprint: None,
            status: ToolStatus::Ok,
            output_bytes: Some(10),
            has_result: true,
        }
    }

    fn data(events: Vec<ToolEvent>) -> ToolData {
        ToolData {
            events,
            has_targets: true,
            has_status: true,
            has_sizes: true,
            ..ToolData::default()
        }
    }

    #[test]
    fn a_read_after_an_edit_is_not_a_repeat() {
        let mut events = Vec::new();
        for i in 0..7 {
            events.push(event(
                i * 2,
                i as i64 * 10,
                "Read",
                ToolKind::Read,
                Some("/a.rs"),
            ));
            if i % 2 == 0 {
                events.push(event(
                    i * 2 + 1,
                    i as i64 * 10 + 5,
                    "Edit",
                    ToolKind::Write,
                    Some("/a.rs"),
                ));
            }
        }
        let a = evaluate(&data(events), &["f".into()], &[], None);
        let r = &a.rules[0];
        assert_eq!(r.status, "checked_none");
        assert!(r.counterexamples[0].starts_with("3 re-reads after an observed edit"));
    }

    #[test]
    fn unexplained_rereads_are_counted_per_range() {
        let mut events: Vec<ToolEvent> = (0..7)
            .map(|i| event(i, i as i64, "Read", ToolKind::Read, Some("/a.rs")))
            .collect();
        // Another area of the same file does not repeat.
        let mut other = event(20, 20, "Read", ToolKind::Read, Some("/a.rs"));
        other.range = Some((100, 50));
        events.push(other);
        let a = evaluate(&data(events), &["f".into()], &[], None);
        let r = &a.rules[0];
        assert_eq!((r.status, r.count, r.of), ("observed", Some(6), Some(8)));
        assert_eq!(r.examples[0].label, "/a.rs");
        assert_eq!(r.evidence[0].file, "f");
    }

    #[test]
    fn a_compaction_between_reads_excuses_the_second() {
        let events: Vec<ToolEvent> = (0..2)
            .map(|i| event(i, i as i64 * 10, "Read", ToolKind::Read, Some("/a.rs")))
            .collect();
        let compaction = Compaction {
            session: "s".into(),
            project: "p".into(),
            source: 0,
            event_id: String::new(),
            time_ms: 5,
            tokens_before: 1,
            tokens_after: None,
            auto: None,
        };
        let a = evaluate(&data(events), &[], &[compaction], None);
        assert!(a.rules[0].counterexamples[1].starts_with("1 re-reads after a context compaction"));
        assert_eq!(a.rules[0].count, Some(0));
    }

    #[test]
    fn identical_failures_count_unless_a_fix_was_observed_between() {
        let mut events = Vec::new();
        for i in 0..5u64 {
            let mut e = event(i, i as i64 * 10, "Bash", ToolKind::Command, None);
            e.fingerprint = Some(7);
            e.status = ToolStatus::Error;
            events.push(e);
        }
        let mut observed = evaluate(&data(events.clone()), &[], &[], None);
        assert_eq!(observed.rules[1].status, "observed");
        assert_eq!(observed.rules[1].count, Some(4));
        assert!(observed.rules[1].examples[0].label.starts_with("Bash #"));
        // An edit between each pair excuses all of them.
        let mut fixed = Vec::new();
        for (i, e) in events.into_iter().enumerate() {
            fixed.push(e);
            fixed.push(event(
                100 + i as u64,
                i as i64 * 10 + 5,
                "Edit",
                ToolKind::Write,
                Some("/x"),
            ));
        }
        observed = evaluate(&data(fixed), &[], &[], None);
        assert_eq!(observed.rules[1].status, "checked_none");
        assert_eq!(observed.rules[1].count, Some(0));
    }

    #[test]
    fn large_outputs_are_bytes_and_a_missing_capability_is_not_measurable() {
        let mut big = event(1, 1, "Bash", ToolKind::Command, None);
        big.output_bytes = Some(250_000);
        let small = event(2, 2, "Read", ToolKind::Read, Some("/a"));
        let a = evaluate(&data(vec![big.clone(), small.clone()]), &[], &[], None);
        assert_eq!(a.rules[2].status, "observed");
        assert_eq!(a.rules[2].count, Some(1));
        let mut d = data(vec![big, small]);
        d.has_targets = false;
        d.has_status = false;
        let a = evaluate(&d, &[], &[], None);
        assert_eq!(a.rules[0].status, "not_measurable");
        assert_eq!(a.rules[1].status, "not_measurable");
        assert_eq!(a.rules[2].status, "observed");
    }

    #[test]
    fn calls_without_a_result_are_reported_not_counted_as_success() {
        let mut e = event(1, 1, "Bash", ToolKind::Command, None);
        e.has_result = false;
        e.output_bytes = None;
        e.status = ToolStatus::Unknown;
        let a = evaluate(&data(vec![e]), &[], &[], None);
        assert_eq!(a.stats.calls_without_result, 1);
        assert_eq!(a.rules[2].status, "not_measurable");
    }
}
