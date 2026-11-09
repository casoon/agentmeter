//! `--transitions`: what the logs show around session starts, resumes, compactions and pauses.
//!
//! One timeline per log file (a context stream), built from metadata only: requests, session
//! starts with the parent the log names, compactions. Nothing is concluded from time alone: a new
//! chat or a pause is a marker, not a loss of context, and neither counts as wasted work. With
//! tool data (`--tool-calls`) the first calls after a compaction are checked for areas read
//! before it; that is a count of what happened, not proof that the re-read was avoidable.

use crate::model::{Compaction, Dataset, Request};
use crate::time::format_iso;
use crate::toolcalls::{ToolData, ToolEvent, ToolKind, ToolStatus};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Lines `(start, end)` of a partial read.
type LineRange = (u64, u64);

/// A gap between two requests of one stream from this long on is listed as a pause.
const PAUSE_MS: i64 = 60 * 60 * 1000;
/// Tool calls after a compaction that are checked for repeated work.
const WINDOW_CALLS: usize = 30;
/// Compactions with re-reads, at least this many, for the project hint.
const HINT_COMPACTIONS: u64 = 2;
const HINT_REREADS: u64 = 5;

#[derive(Debug, Clone, Serialize)]
pub struct Parent {
    pub id: String,
    /// The parent's own log is among the logs read.
    pub in_logs: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Repeats {
    /// What was checked, in words.
    pub window: String,
    pub calls_checked: u64,
    pub reads: u64,
    /// Reads of an area (file and line range) that was read before the compaction and not
    /// edited since.
    pub reread_areas: u64,
    /// Calls that repeat a failed call from before the compaction unchanged.
    pub failures_repeated: u64,
    /// Up to three of the re-read files.
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transition {
    /// `start`, `resumed_or_forked`, `incomplete_log`, `compaction` or `pause`.
    pub kind: &'static str,
    pub project: String,
    pub session: String,
    pub time: String,
    /// The log file and the event id (compaction) the entry is based on.
    pub file: String,
    pub event: Option<String>,
    /// Context of the request before the event (compaction: as the log states it).
    pub context_before: Option<u64>,
    /// Compaction only: size after, as the log states it; `None` when it does not.
    pub context_after: Option<u64>,
    /// Context of the first request after the event.
    pub next_request_context: Option<u64>,
    pub ms_to_next_request: Option<i64>,
    /// Compaction only: requests of the stream until the next compaction or the log's end.
    pub requests_to_next_compaction: Option<u64>,
    /// Compaction only: no further compaction follows, the count runs to the end of the log.
    pub then_log_ends: Option<bool>,
    pub parent: Option<Parent>,
    /// Compaction only, and only with tool data.
    pub repeats: Option<Repeats>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Counts {
    pub starts: u64,
    pub resumed_or_forked: u64,
    pub incomplete_log: u64,
    pub compactions: u64,
    pub pauses: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectTransitions {
    pub project: String,
    pub counts: Counts,
    pub median_context_before_compaction: Option<u64>,
    /// Compactions whose follow-up window could be checked (tool data present).
    pub windows_checked: u64,
    pub compactions_with_rereads: u64,
    pub rereads_after_compaction: u64,
    /// Worded from the pattern above; `null` when there is none.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransitionReport {
    pub method: &'static str,
    /// Tool calls were read (`--tool-calls`), so compactions carry `repeats`.
    pub tool_data: bool,
    pub counts: Counts,
    pub projects: Vec<ProjectTransitions>,
    pub transitions: Vec<Transition>,
    pub notes: Vec<String>,
}

const METHOD: &str = "one timeline per log file from request times, the parent a log names for a \
                      resumed or forked session, and compactions; time alone never makes a \
                      transition a loss of context";

fn median(mut v: Vec<u64>) -> Option<u64> {
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

/// Builds the report for the transitions at or after `since_ms`.
pub fn analyse(
    dataset: &Dataset,
    tools: Option<&ToolData>,
    since_ms: Option<i64>,
) -> TransitionReport {
    let in_period = |t: i64| since_ms.is_none_or(|s| t >= s);
    // Main-conversation requests per stream, in time order.
    let mut streams: BTreeMap<usize, Vec<&Request>> = BTreeMap::new();
    for r in dataset.requests.iter().filter(|r| !r.sub) {
        streams.entry(r.source).or_default().push(r);
    }
    for v in streams.values_mut() {
        v.sort_by_key(|r| r.time_ms);
    }
    let file = |source: usize| dataset.files.get(source).cloned().unwrap_or_default();
    let known: HashSet<&str> = dataset
        .session_starts
        .iter()
        .map(|s| s.session.as_str())
        .collect();
    let mut transitions: Vec<Transition> = Vec::new();
    let mut notes = Vec::new();
    let mut starts_without_requests = 0u64;

    for start in &dataset.session_starts {
        let Some(first) = streams.get(&start.source).and_then(|v| v.first()) else {
            starts_without_requests += 1;
            continue;
        };
        if !in_period(first.time_ms) {
            continue;
        }
        let kind = match start.reason {
            None => "start",
            Some("inherited-history") => "resumed_or_forked",
            _ => "incomplete_log",
        };
        let parent = start
            .parent
            .as_ref()
            .filter(|_| kind == "resumed_or_forked")
            .map(|id| Parent {
                id: id.clone(),
                in_logs: known.contains(id.as_str()),
            });
        transitions.push(Transition {
            kind,
            project: first.project.clone(),
            session: start.session.clone(),
            time: format_iso(first.time_ms),
            file: file(start.source),
            event: None,
            context_before: None,
            context_after: None,
            next_request_context: Some(first.context()),
            ms_to_next_request: None,
            requests_to_next_compaction: None,
            then_log_ends: None,
            parent,
            repeats: None,
        });
    }

    // Pauses between two requests of one stream.
    for (source, requests) in &streams {
        for pair in requests.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b.time_ms - a.time_ms >= PAUSE_MS && in_period(b.time_ms) {
                transitions.push(Transition {
                    kind: "pause",
                    project: a.project.clone(),
                    session: a.session.clone(),
                    time: format_iso(a.time_ms),
                    file: file(*source),
                    event: None,
                    context_before: Some(a.context()),
                    context_after: None,
                    next_request_context: Some(b.context()),
                    ms_to_next_request: Some(b.time_ms - a.time_ms),
                    requests_to_next_compaction: None,
                    then_log_ends: None,
                    parent: None,
                    repeats: None,
                });
            }
        }
    }

    // Compactions, each with what the stream shows before and after.
    let mut by_stream: BTreeMap<usize, Vec<&Compaction>> = BTreeMap::new();
    for c in &dataset.compactions {
        by_stream.entry(c.source).or_default().push(c);
    }
    let mut tool_events: HashMap<usize, Vec<&ToolEvent>> = HashMap::new();
    if let Some(data) = tools {
        for e in &data.events {
            tool_events.entry(e.source).or_default().push(e);
        }
        for v in tool_events.values_mut() {
            v.sort_by_key(|e| (e.time_ms, e.seq));
        }
    }
    for (source, mut list) in by_stream {
        list.sort_by_key(|c| c.time_ms);
        let requests = streams.get(&source).map(Vec::as_slice).unwrap_or(&[]);
        for (i, c) in list.iter().enumerate() {
            if !in_period(c.time_ms) {
                continue;
            }
            let next_c = list.get(i + 1).map(|n| n.time_ms);
            let next_request = requests.iter().find(|r| r.time_ms > c.time_ms);
            let until = next_c.unwrap_or(i64::MAX);
            let following = requests
                .iter()
                .filter(|r| r.time_ms > c.time_ms && r.time_ms < until)
                .count() as u64;
            let repeats = tools.map(|_| {
                repeats_after(
                    tool_events.get(&source).map_or(&[][..], Vec::as_slice),
                    c.time_ms,
                    until,
                )
            });
            transitions.push(Transition {
                kind: "compaction",
                project: c.project.clone(),
                session: c.session.clone(),
                time: format_iso(c.time_ms),
                file: file(source),
                event: Some(c.event_id.clone()).filter(|e| !e.is_empty()),
                context_before: Some(c.tokens_before),
                context_after: c.tokens_after,
                next_request_context: next_request.map(|r| r.context()),
                ms_to_next_request: next_request.map(|r| r.time_ms - c.time_ms),
                requests_to_next_compaction: Some(following),
                then_log_ends: Some(next_c.is_none()),
                parent: None,
                repeats,
            });
        }
    }
    transitions.sort_by(|a, b| (&a.time, &a.file, a.kind).cmp(&(&b.time, &b.file, b.kind)));

    let mut counts = Counts::default();
    let mut per_project: BTreeMap<&str, Vec<&Transition>> = BTreeMap::new();
    for t in &transitions {
        bump(&mut counts, t.kind);
        per_project.entry(t.project.as_str()).or_default().push(t);
    }
    let mut projects: Vec<ProjectTransitions> = per_project
        .into_iter()
        .map(|(project, list)| project_row(project, &list))
        .collect();
    projects.sort_by(|a, b| {
        (b.counts.compactions, &a.project).cmp(&(a.counts.compactions, &b.project))
    });
    if starts_without_requests > 0 {
        notes.push(format!(
            "{starts_without_requests} log file(s) without usage have no start in this view"
        ));
    }
    notes.push(
        "A start, a resume or fork, a pause or the end of a log is a marker, not proof of a crash, \
         an abort or lost context. Sessions in one project are not linked by time; only a parent \
         named in the log links two sessions."
            .to_owned(),
    );
    notes.push(
        "A resume copies earlier history into a new file; its start is the first request of the \
         file, and tool repeats are checked after compactions only."
            .to_owned(),
    );
    notes.push(
        "Consumption after a transition is measured, not proven avoidable; compactions are the \
         same events the summary counts."
            .to_owned(),
    );
    TransitionReport {
        method: METHOD,
        tool_data: tools.is_some(),
        counts,
        projects,
        transitions,
        notes,
    }
}

fn bump(counts: &mut Counts, kind: &str) {
    match kind {
        "start" => counts.starts += 1,
        "resumed_or_forked" => counts.resumed_or_forked += 1,
        "incomplete_log" => counts.incomplete_log += 1,
        "compaction" => counts.compactions += 1,
        _ => counts.pauses += 1,
    }
}

fn project_row(project: &str, list: &[&Transition]) -> ProjectTransitions {
    let mut counts = Counts::default();
    let mut before = Vec::new();
    let (mut checked, mut with, mut total) = (0, 0, 0);
    for t in list {
        bump(&mut counts, t.kind);
        if t.kind == "compaction" {
            before.extend(t.context_before);
            if let Some(r) = &t.repeats {
                checked += 1;
                if r.reread_areas > 0 {
                    with += 1;
                    total += r.reread_areas;
                }
            }
        }
    }
    let hint = (with >= HINT_COMPACTIONS && total >= HINT_REREADS).then(|| {
        format!(
            "After {with} of {checked} compactions the next calls read areas again that had been \
             read before; a short note of the files that matter, or compacting with instructions \
             that name them, may save re-reading. The logs do not show whether the re-reads were \
             needed."
        )
    });
    ProjectTransitions {
        project: project.to_owned(),
        counts,
        median_context_before_compaction: median(before),
        windows_checked: checked,
        compactions_with_rereads: with,
        rereads_after_compaction: total,
        hint,
    }
}

/// The first calls of a stream after a compaction (not past the next one), checked against what
/// the stream did before it.
fn repeats_after(events: &[&ToolEvent], at_ms: i64, until_ms: i64) -> Repeats {
    let before = || events.iter().filter(|e| e.time_ms < at_ms);
    let window: Vec<&&ToolEvent> = events
        .iter()
        .filter(|e| e.time_ms > at_ms && e.time_ms < until_ms)
        .take(WINDOW_CALLS)
        .collect();
    let mut last_read: HashMap<(&str, Option<LineRange>), &ToolEvent> = HashMap::new();
    let mut last_failure: HashMap<(&str, u64), &ToolEvent> = HashMap::new();
    for e in before() {
        if e.kind == ToolKind::Read
            && let Some(t) = e.target.as_deref()
        {
            last_read.insert((t, e.range), e);
        }
        if e.status == ToolStatus::Error
            && let Some(f) = e.fingerprint
        {
            last_failure.insert((e.tool.as_str(), f), e);
        }
    }
    let edited_between = |path: Option<&str>, from: &ToolEvent, to: &ToolEvent| {
        events.iter().any(|w| {
            w.kind == ToolKind::Write
                && (w.time_ms, w.seq) > (from.time_ms, from.seq)
                && (w.time_ms, w.seq) < (to.time_ms, to.seq)
                && path.is_none_or(|p| w.target.as_deref() == Some(p))
        })
    };
    let (mut reads, mut reread, mut failures) = (0, 0, 0);
    let mut examples: Vec<String> = Vec::new();
    for e in window.iter().map(|e| **e) {
        if e.kind == ToolKind::Read
            && let Some(t) = e.target.as_deref()
        {
            reads += 1;
            if let Some(prev) = last_read.get(&(t, e.range))
                && !edited_between(Some(t), prev, e)
            {
                reread += 1;
                if examples.len() < 3 && !examples.iter().any(|x| x == t) {
                    examples.push(t.to_owned());
                }
            }
        }
        if let Some(f) = e.fingerprint
            && let Some(prev) = last_failure.get(&(e.tool.as_str(), f))
            && !edited_between(None, prev, e)
        {
            failures += 1;
        }
    }
    Repeats {
        window: format!(
            "the first {WINDOW_CALLS} tool calls after the compaction, not past the next one"
        ),
        calls_checked: window.len() as u64,
        reads,
        reread_areas: reread,
        failures_repeated: failures,
        examples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionStart;

    fn req(source: usize, t: i64, ctx: u64) -> Request {
        Request {
            session: format!("s{source}"),
            project: "p".into(),
            source,
            message_id: format!("m{t}"),
            time_ms: t,
            model: "m".into(),
            sub: false,
            fresh: ctx,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            output: 1,
            window: None,
        }
    }

    fn start(source: usize, reason: Option<&'static str>, parent: Option<&str>) -> SessionStart {
        SessionStart {
            session: format!("s{source}"),
            source,
            clean: reason.is_none(),
            reason,
            entrypoint: None,
            cwd: None,
            parent: parent.map(str::to_owned),
        }
    }

    fn compaction(source: usize, t: i64, after: Option<u64>) -> Compaction {
        Compaction {
            session: format!("s{source}"),
            project: "p".into(),
            source,
            event_id: format!("c{t}"),
            time_ms: t,
            tokens_before: 900,
            tokens_after: after,
            auto: None,
        }
    }

    #[test]
    fn starts_forks_incomplete_logs_and_pauses_are_told_apart() {
        let day = 24 * 3600 * 1000;
        let dataset = Dataset {
            requests: vec![
                req(0, 1000, 10),
                req(0, 2000 + 2 * 3600 * 1000, 20),
                req(1, day, 30),
                req(2, day, 40),
            ],
            files: vec!["a".into(), "b".into(), "c".into()],
            session_starts: vec![
                start(0, None, None),
                start(1, Some("inherited-history"), Some("s0")),
                start(2, Some("no-log-start"), None),
            ],
            ..Dataset::default()
        };
        let r = analyse(&dataset, None, None);
        assert_eq!(
            (
                r.counts.starts,
                r.counts.resumed_or_forked,
                r.counts.incomplete_log
            ),
            (1, 1, 1)
        );
        assert_eq!(r.counts.pauses, 1);
        let fork = r
            .transitions
            .iter()
            .find(|t| t.kind == "resumed_or_forked")
            .unwrap();
        let parent = fork.parent.as_ref().unwrap();
        assert!(parent.in_logs && parent.id == "s0");
        // The pause is a marker with both contexts, no verdict.
        let pause = r.transitions.iter().find(|t| t.kind == "pause").unwrap();
        assert_eq!(
            (pause.context_before, pause.next_request_context),
            (Some(10), Some(20))
        );
        assert!(pause.repeats.is_none());
        // A new chat close in time is not linked to anything.
        assert!(
            r.transitions
                .iter()
                .filter(|t| t.kind == "start")
                .all(|t| t.parent.is_none())
        );
    }

    #[test]
    fn compactions_keep_unknown_sizes_unknown_and_count_to_the_next_one() {
        let dataset = Dataset {
            requests: (1..=6).map(|i| req(0, i * 1000, 100)).collect(),
            compactions: vec![compaction(0, 1500, None), compaction(0, 3500, Some(50))],
            files: vec!["a".into()],
            session_starts: vec![start(0, None, None)],
            ..Dataset::default()
        };
        let r = analyse(&dataset, None, None);
        let c: Vec<&Transition> = r
            .transitions
            .iter()
            .filter(|t| t.kind == "compaction")
            .collect();
        assert_eq!(r.counts.compactions, 2);
        assert_eq!(c[0].context_after, None);
        assert_eq!(c[1].context_after, Some(50));
        assert_eq!(
            (c[0].requests_to_next_compaction, c[0].then_log_ends),
            (Some(2), Some(false))
        );
        assert_eq!(
            (c[1].requests_to_next_compaction, c[1].then_log_ends),
            (Some(3), Some(true))
        );
        assert_eq!(c[0].ms_to_next_request, Some(500));
        assert!(c[0].repeats.is_none());
    }

    fn call(seq: u64, t: i64, kind: ToolKind, tool: &str, target: Option<&str>) -> ToolEvent {
        ToolEvent {
            source: 0,
            session: "s0".into(),
            project: "p".into(),
            sub: false,
            seq,
            time_ms: t,
            call_id: format!("id{seq}"),
            tool: tool.into(),
            kind,
            target: target.map(str::to_owned),
            range: None,
            fingerprint: None,
            status: ToolStatus::Ok,
            output_bytes: Some(1),
            has_result: true,
        }
    }

    #[test]
    fn rereads_after_a_compaction_are_counted_unless_the_file_changed() {
        let mut events = vec![
            call(1, 100, ToolKind::Read, "Read", Some("/a")),
            call(2, 110, ToolKind::Read, "Read", Some("/b")),
            call(3, 120, ToolKind::Write, "Edit", Some("/b")),
            // after the compaction at 500:
            call(4, 600, ToolKind::Read, "Read", Some("/a")),
            call(5, 610, ToolKind::Read, "Read", Some("/b")),
            call(6, 620, ToolKind::Read, "Read", Some("/new")),
        ];
        let mut failed = call(0, 130, ToolKind::Command, "Bash", None);
        failed.fingerprint = Some(9);
        failed.status = ToolStatus::Error;
        let mut again = call(7, 630, ToolKind::Command, "Bash", None);
        again.fingerprint = Some(9);
        again.status = ToolStatus::Error;
        events.push(failed);
        events.push(again);
        let tools = ToolData {
            events,
            has_targets: true,
            has_status: true,
            has_sizes: true,
            ..ToolData::default()
        };
        let dataset = Dataset {
            requests: vec![req(0, 50, 10), req(0, 700, 10)],
            compactions: vec![compaction(0, 500, None)],
            files: vec!["a".into()],
            session_starts: vec![start(0, None, None)],
            ..Dataset::default()
        };
        let r = analyse(&dataset, Some(&tools), None);
        let rep = r
            .transitions
            .iter()
            .find(|t| t.kind == "compaction")
            .unwrap()
            .repeats
            .as_ref()
            .unwrap();
        // /a again (unchanged): counted. /b was edited after its first read, so the earlier read
        // is not the same state: not counted. /new is new.
        assert_eq!(
            (rep.reads, rep.reread_areas, rep.failures_repeated),
            (3, 1, 1)
        );
        assert_eq!(rep.examples, ["/a"]);
        assert_eq!(r.projects[0].windows_checked, 1);
    }

    #[test]
    fn the_period_limits_transitions() {
        let dataset = Dataset {
            requests: vec![req(0, 100, 1), req(0, 900, 1)],
            compactions: vec![compaction(0, 200, None), compaction(0, 800, None)],
            files: vec!["a".into()],
            session_starts: vec![start(0, None, None)],
            ..Dataset::default()
        };
        let r = analyse(&dataset, None, Some(500));
        assert_eq!(r.counts.compactions, 1);
        assert_eq!(r.counts.starts, 0);
    }
}
