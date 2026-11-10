//! `--subagents`: which consumption belongs to main sessions and which to delegated agents, which
//! streams were active at the same time, and which files several of them touched.
//!
//! Only parents the logs name are used (Claude Code: the session folder of a sub-agent's log;
//! Codex: the spawn record, the fork id, or the parent history copied into the rollout). A
//! project name or closeness in time never makes a delegation: an agent without a named parent
//! stays unassigned. Requests are partitioned, never added twice: direct requests of sessions,
//! requests of assigned agents (nested ones included once, under their root session) and
//! unassigned agent requests add up to the requests the summary counts.
//!
//! Time comes from request timestamps: a run is a stretch of requests that are less than the pause
//! limit of `--transitions` apart; its length (first to last request) still contains waiting, and
//! a run of one request has no observed length. Overlap of runs is a reason to look, not proof of
//! conflicting work.
//!
//! Claude Code also logs how long a finished run of an agent took (`logged_run_ms`): wall time of
//! that run, waiting included. It is known for agents that ran in the foreground, and for
//! background agents when the notice of their end was read with the tool calls; elsewhere it
//! stays unknown.

use crate::model::{AgentLink, Dataset, Request};
use crate::time::format_iso;
use crate::toolcalls::{ToolData, ToolEvent, ToolKind};
use crate::transitions::PAUSE_MS;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Lines `(start, end)` of a partial read.
type LineRange = (u64, u64);
/// One stream of requests: a log file and, for a main session, the session id.
type StreamKey = (usize, bool, String);
/// Time two streams were both in a run, and the windows it came from.
type PairOverlap = (i64, Vec<(i64, i64)>);

/// Overlaps listed in the report; the counts cover all of them.
const MAX_OVERLAPS: usize = 20;
const MAX_EXAMPLES: usize = 3;

const METHOD: &str = "requests split into the direct ones of a session and those of agents that \
                      the log names a parent for; overlap from the runs of requests (less than \
                      an hour apart); shared files from tool calls; no project or time link makes \
                      a delegation";

#[derive(Debug, Default, Clone, Serialize)]
pub struct Usage {
    pub requests: u64,
    /// `fresh + cache_read + cache_write`, as in the summary.
    pub input: u64,
    pub fresh: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// Requests per model; a request without a model name counts as `unknown`.
    pub models: BTreeMap<String, u64>,
}

impl Usage {
    fn add(&mut self, r: &Request) {
        self.requests += 1;
        self.input += r.context();
        self.fresh += r.fresh;
        self.cache_read += r.cache_read;
        self.cache_write += r.cache_write();
        self.output += r.output;
        let model = if r.model.is_empty() {
            "unknown"
        } else {
            r.model.as_str()
        };
        *self.models.entry(model.to_owned()).or_default() += 1;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Child {
    /// The agent's id as the log gives it; `null` when the stream has no agent log of its own.
    pub agent: Option<String>,
    /// The session or agent the log names as its parent; `null` when it names none.
    pub parent: Option<String>,
    /// Where the log names the parent; `null` with the parent.
    pub parent_basis: Option<&'static str>,
    /// Steps from the root session along named parents (1 is a direct child); `null` when the
    /// chain does not reach a session.
    pub depth: Option<u64>,
    pub project: String,
    pub file: String,
    pub first: String,
    pub last: String,
    /// Finished runs of the agent in the period whose duration the log records.
    pub logged_runs: u64,
    /// Their durations added up, milliseconds: wall time, waiting included; `null` without one.
    pub logged_run_ms: Option<u64>,
    pub usage: Usage,
}

/// Context the first request of each delegated agent sent; only for suitable data.
#[derive(Debug, Clone, Serialize)]
pub struct StartContext {
    pub agents: u64,
    pub context: u64,
    pub fresh: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Root {
    pub session: String,
    pub project: String,
    /// The session's own requests are among the requests read.
    pub in_logs: bool,
    pub direct: Usage,
    /// Requests of every agent assigned to this session, nested ones included once.
    pub delegated: Usage,
    /// `direct.input + delegated.input`, derived for reading; never added to a total.
    pub inclusive_input: u64,
    pub agents: u64,
    pub max_depth: u64,
    pub first: String,
    pub last: String,
    /// Last minus first request of session and agents, milliseconds.
    pub span_ms: i64,
    /// Sum of the lengths of the runs; waiting inside a run is included.
    pub active_ms: i64,
    /// Gaps of the pause limit or more between runs.
    pub paused_ms: i64,
    pub runs: u64,
    /// Agents of this session with a logged run time.
    pub agents_with_logged_run: u64,
    /// Their logged run times added up; agents can run at once, so this is not elapsed time.
    /// `null` when no agent has one.
    pub logged_run_ms: Option<u64>,
    /// Areas (file and line range) read by two or more streams of this session; `null` when
    /// file targets are not measurable.
    pub shared_reads: Option<u64>,
    /// Files read by two or more streams but never the same lines (counter-example).
    pub shared_reads_other_ranges: Option<u64>,
    /// Files written by two or more streams.
    pub shared_writes: Option<u64>,
    /// Up to three file paths among the shared ones.
    pub shared_examples: Vec<String>,
    pub start_context: Option<StartContext>,
    pub children: Vec<Child>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overlap {
    pub first: String,
    /// `session` or `agent`.
    pub first_kind: &'static str,
    pub second: String,
    pub second_kind: &'static str,
    /// `delegation` (one is an ancestor of the other by named parents), `same_task` (agents of
    /// one session) or `time_only` (nothing in the logs relates them).
    pub relation: &'static str,
    /// Time both were in a run, milliseconds; 0 when one run lies inside a single request.
    pub overlap_ms: i64,
    /// Areas both read during the overlap; `null` when not measurable.
    pub shared_reads: Option<u64>,
    /// Files both wrote during the overlap; `null` when not measurable.
    pub shared_writes: Option<u64>,
    /// Up to three of those files.
    pub examples: Vec<String>,
    /// The log files the pair is based on.
    pub files: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Counts {
    pub sessions: u64,
    pub sessions_with_agents: u64,
    pub agents: u64,
    pub nested_agents: u64,
    pub max_depth: u64,
    pub unassigned_agents: u64,
    /// Agents with at least one run whose duration the log records.
    pub agents_with_logged_run: u64,
    /// Logged run durations that name no agent in the data read.
    pub run_records_unlinked: u64,
    pub overlaps_delegation: u64,
    pub overlaps_same_task: u64,
    pub overlaps_time_only: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    /// Requests and input of the period, counted once.
    pub requests: u64,
    pub input: u64,
    /// Requests of main sessions.
    pub direct: Usage,
    /// Requests of agents with a named parent that leads to a session.
    pub delegated: Usage,
    /// Agent requests whose parent the logs do not name.
    pub unassigned: Usage,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubagentReport {
    pub method: &'static str,
    /// File targets of tool calls were read and the log records them.
    pub file_data: bool,
    pub counts: Counts,
    pub totals: Totals,
    /// Sessions that delegated, busiest first.
    pub roots: Vec<Root>,
    /// Agents without a usable parent chain; same fields as `children`.
    pub unassigned: Vec<Child>,
    pub overlaps: Vec<Overlap>,
    pub counterexamples: Vec<String>,
    pub notes: Vec<String>,
}

struct Stream<'a> {
    key: StreamKey,
    id: String,
    kind: &'static str,
    project: String,
    file: String,
    link: Option<&'a AgentLink>,
    root: Option<String>,
    /// Named parents upward, ending at the root session.
    ancestry: Vec<String>,
    requests: Vec<&'a Request>,
}

/// Follows the named parents of an agent up to a session. `None` when a parent is missing along
/// the way or the chain loops.
fn resolve(link: &AgentLink, by_agent: &HashMap<&str, &AgentLink>) -> Option<Vec<String>> {
    let mut ancestry = Vec::new();
    let mut seen = HashSet::from([link.agent.as_str()]);
    let mut current = link;
    loop {
        let parent = current.parent.as_deref()?;
        ancestry.push(parent.to_owned());
        match by_agent.get(parent) {
            Some(next) => {
                if !seen.insert(next.agent.as_str()) {
                    return None;
                }
                current = next;
            }
            None => return Some(ancestry),
        }
    }
}

/// Runs of one sorted list of times: a gap of the pause limit or more starts a new run.
fn runs(times: &[i64]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &t in times {
        match out.last_mut() {
            Some(run) if t - run.1 < PAUSE_MS => run.1 = t,
            _ => out.push((t, t)),
        }
    }
    out
}

fn relation(a: &Stream, b: &Stream) -> Option<&'static str> {
    if !a.key.1 && !b.key.1 {
        // Two main streams: one session's copies are no pair, different sessions only coincide.
        return (a.id != b.id).then_some("time_only");
    }
    Some(match (&a.root, &b.root) {
        (Some(x), Some(y)) if x == y => {
            if a.ancestry.contains(&b.id) || b.ancestry.contains(&a.id) {
                "delegation"
            } else {
                "same_task"
            }
        }
        _ => "time_only",
    })
}

type ReadArea<'a> = (&'a str, Option<LineRange>);

/// File accesses of one stream inside the given time windows.
fn touched<'a>(
    events: &[&'a ToolEvent],
    windows: &[(i64, i64)],
) -> (HashSet<ReadArea<'a>>, HashSet<&'a str>) {
    let mut reads = HashSet::new();
    let mut writes = HashSet::new();
    for e in events {
        let Some(target) = e.target.as_deref() else {
            continue;
        };
        if !windows
            .iter()
            .any(|(s, t)| e.time_ms >= *s && e.time_ms <= *t)
        {
            continue;
        }
        match e.kind {
            ToolKind::Read => {
                reads.insert((target, e.range));
            }
            ToolKind::Write => {
                writes.insert(target);
            }
            _ => {}
        }
    }
    (reads, writes)
}

fn examples<'a>(
    writes: impl Iterator<Item = &'a str>,
    reads: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut writes: Vec<&str> = writes.collect();
    writes.sort_unstable();
    let mut reads: Vec<&str> = reads.collect();
    reads.sort_unstable();
    writes
        .into_iter()
        .chain(reads)
        .filter(|p| seen.insert(*p))
        .take(MAX_EXAMPLES)
        .map(str::to_owned)
        .collect()
}

/// Builds the report for the requests at or after `since_ms`. `tools` is present with
/// `--tool-calls`.
pub fn analyse(
    dataset: &Dataset,
    tools: Option<&ToolData>,
    since_ms: Option<i64>,
) -> SubagentReport {
    let file = |source: usize| dataset.files.get(source).cloned().unwrap_or_default();
    let by_source: HashMap<usize, &AgentLink> =
        dataset.agent_links.iter().map(|l| (l.source, l)).collect();
    let mut by_agent: HashMap<&str, &AgentLink> = HashMap::new();
    for link in &dataset.agent_links {
        by_agent.entry(link.agent.as_str()).or_insert(link);
    }

    let mut grouped: BTreeMap<StreamKey, Vec<&Request>> = BTreeMap::new();
    for r in dataset
        .requests
        .iter()
        .filter(|r| since_ms.is_none_or(|s| r.time_ms >= s))
    {
        let key = if r.sub {
            (r.source, true, String::new())
        } else {
            (r.source, false, r.session.clone())
        };
        grouped.entry(key).or_default().push(r);
    }
    let mut streams: Vec<Stream> = Vec::new();
    for (key, mut requests) in grouped {
        requests.sort_by_key(|r| r.time_ms);
        let project = requests[0].project.clone();
        let mut stream = Stream {
            id: key.2.clone(),
            kind: "session",
            project,
            file: file(key.0),
            link: None,
            root: None,
            ancestry: Vec::new(),
            requests,
            key,
        };
        if stream.key.1 {
            stream.kind = "agent";
            stream.link = by_source.get(&stream.key.0).copied();
            match stream.link {
                Some(link) => {
                    stream.id = link.agent.clone();
                    if let Some(ancestry) = resolve(link, &by_agent) {
                        stream.root = ancestry.last().cloned();
                        stream.ancestry = ancestry;
                    }
                }
                None => stream.id = stream.file.clone(),
            }
        } else {
            stream.root = Some(stream.id.clone());
        }
        streams.push(stream);
    }

    // Partition of the requests: direct, delegated to a root, unassigned.
    let mut totals = Totals {
        requests: streams.iter().map(|s| s.requests.len() as u64).sum(),
        input: streams
            .iter()
            .flat_map(|s| s.requests.iter())
            .map(|r| r.context())
            .sum(),
        direct: Usage::default(),
        delegated: Usage::default(),
        unassigned: Usage::default(),
    };
    struct Acc {
        project: String,
        in_logs: bool,
        direct: Usage,
        delegated: Usage,
        children: Vec<Child>,
        streams: Vec<usize>,
    }
    let mut accs: BTreeMap<String, Acc> = BTreeMap::new();
    let mut unassigned: Vec<Child> = Vec::new();
    let mut logged: HashMap<&str, (u64, u64)> = HashMap::new();
    for run in dataset
        .agent_runs
        .iter()
        .filter(|r| since_ms.is_none_or(|s| r.end_ms >= s))
    {
        let entry = logged.entry(run.agent.as_str()).or_default();
        entry.0 += 1;
        entry.1 += run.duration_ms;
    }
    let child_of = |s: &Stream, usage: Usage| {
        let runs = s.link.and_then(|l| logged.get(l.agent.as_str())).copied();
        Child {
            logged_runs: runs.map_or(0, |r| r.0),
            logged_run_ms: runs.map(|r| r.1),
            agent: s.link.map(|l| l.agent.clone()),
            parent: s.link.and_then(|l| l.parent.clone()),
            parent_basis: s.link.and_then(|l| l.basis),
            depth: s.root.as_ref().map(|_| s.ancestry.len() as u64),
            project: s.project.clone(),
            file: s.file.clone(),
            first: format_iso(s.requests[0].time_ms),
            last: format_iso(s.requests[s.requests.len() - 1].time_ms),
            usage,
        }
    };
    for (i, s) in streams.iter().enumerate() {
        let mut usage = Usage::default();
        for r in &s.requests {
            usage.add(r);
        }
        let Some(root) = s.root.clone() else {
            for r in &s.requests {
                totals.unassigned.add(r);
            }
            unassigned.push(child_of(s, usage));
            continue;
        };
        let acc = accs.entry(root).or_insert_with(|| Acc {
            project: s.project.clone(),
            in_logs: false,
            direct: Usage::default(),
            delegated: Usage::default(),
            children: Vec::new(),
            streams: Vec::new(),
        });
        acc.streams.push(i);
        if s.key.1 {
            for r in &s.requests {
                totals.delegated.add(r);
                acc.delegated.add(r);
            }
            acc.children.push(child_of(s, usage));
        } else {
            for r in &s.requests {
                totals.direct.add(r);
                acc.direct.add(r);
            }
            acc.in_logs = true;
            acc.project = s.project.clone();
        }
    }

    // Tool events by stream, when the log records what they touch.
    let file_data = tools.is_some_and(|t| t.has_targets);
    let index: HashMap<&StreamKey, usize> = streams
        .iter()
        .enumerate()
        .map(|(i, s)| (&s.key, i))
        .collect();
    let mut events: Vec<Vec<&ToolEvent>> = vec![Vec::new(); streams.len()];
    if let Some(data) = tools.filter(|_| file_data) {
        for e in data
            .events
            .iter()
            .filter(|e| since_ms.is_none_or(|s| e.time_ms >= s))
        {
            let key = if e.sub {
                (e.source, true, String::new())
            } else {
                (e.source, false, e.session.clone())
            };
            if let Some(&i) = index.get(&key) {
                events[i].push(e);
            }
        }
    }

    // Overlap of runs, pair by pair.
    struct Run {
        start: i64,
        end: i64,
        stream: usize,
    }
    let mut all: Vec<Run> = Vec::new();
    for (i, s) in streams.iter().enumerate() {
        let times: Vec<i64> = s.requests.iter().map(|r| r.time_ms).collect();
        all.extend(runs(&times).into_iter().map(|(start, end)| Run {
            start,
            end,
            stream: i,
        }));
    }
    all.sort_by_key(|r| (r.start, r.end, r.stream));
    let mut pairs: BTreeMap<(usize, usize), PairOverlap> = BTreeMap::new();
    let mut active: Vec<usize> = Vec::new();
    for i in 0..all.len() {
        active.retain(|&j| all[j].end >= all[i].start);
        for &j in &active {
            let (a, b) = (all[j].stream, all[i].stream);
            if a == b || relation(&streams[a], &streams[b]).is_none() {
                continue;
            }
            let end = all[i].end.min(all[j].end);
            let entry = pairs.entry((a.min(b), a.max(b))).or_default();
            entry.0 += end - all[i].start;
            entry.1.push((all[i].start, end));
        }
        active.push(i);
    }
    let mut counts = Counts::default();
    let mut overlaps: Vec<Overlap> = Vec::new();
    for ((a, b), (overlap_ms, windows)) in pairs {
        let (sa, sb) = (&streams[a], &streams[b]);
        let relation = relation(sa, sb).unwrap_or("time_only");
        match relation {
            "delegation" => counts.overlaps_delegation += 1,
            "same_task" => counts.overlaps_same_task += 1,
            _ => counts.overlaps_time_only += 1,
        }
        let (mut shared_reads, mut shared_writes, mut example_files) = (None, None, Vec::new());
        if file_data {
            let (ra, wa) = touched(&events[a], &windows);
            let (rb, wb) = touched(&events[b], &windows);
            let reads: Vec<&ReadArea> = ra.intersection(&rb).collect();
            let writes: Vec<&&str> = wa.intersection(&wb).collect();
            shared_reads = Some(reads.len() as u64);
            shared_writes = Some(writes.len() as u64);
            example_files = examples(writes.iter().map(|p| **p), reads.iter().map(|(p, _)| *p));
        }
        overlaps.push(Overlap {
            first: sa.id.clone(),
            first_kind: sa.kind,
            second: sb.id.clone(),
            second_kind: sb.kind,
            relation,
            overlap_ms,
            shared_reads,
            shared_writes,
            examples: example_files,
            files: vec![sa.file.clone(), sb.file.clone()],
        });
    }
    let overlap_total = overlaps.len();
    overlaps.sort_by(|x, y| {
        (
            std::cmp::Reverse(x.shared_writes),
            std::cmp::Reverse(x.shared_reads),
            std::cmp::Reverse(x.overlap_ms),
            &x.first,
            &x.second,
        )
            .cmp(&(
                std::cmp::Reverse(y.shared_writes),
                std::cmp::Reverse(y.shared_reads),
                std::cmp::Reverse(y.overlap_ms),
                &y.first,
                &y.second,
            ))
    });
    overlaps.truncate(MAX_OVERLAPS);

    // Roots that delegated.
    let mut roots: Vec<Root> = Vec::new();
    let mut other_range_files = 0u64;
    for (session, acc) in accs {
        if acc.children.is_empty() {
            continue;
        }
        let mut times: Vec<i64> = acc
            .streams
            .iter()
            .flat_map(|&i| streams[i].requests.iter().map(|r| r.time_ms))
            .collect();
        times.sort_unstable();
        let span = times.last().unwrap_or(&0) - times.first().unwrap_or(&0);
        let paused: i64 = times
            .windows(2)
            .map(|w| w[1] - w[0])
            .filter(|gap| *gap >= PAUSE_MS)
            .sum();
        let run_count = runs(&times).len() as u64;

        let mut start_context = None;
        let firsts: Vec<&Request> = acc
            .streams
            .iter()
            .filter(|&&i| streams[i].key.1)
            .map(|&i| streams[i].requests[0])
            .collect();
        if firsts.len() >= 2 && dataset.stats.cache_write_measured {
            start_context = Some(StartContext {
                agents: firsts.len() as u64,
                context: firsts.iter().map(|r| r.context()).sum(),
                fresh: firsts.iter().map(|r| r.fresh).sum(),
                cache_read: firsts.iter().map(|r| r.cache_read).sum(),
                cache_write: firsts.iter().map(|r| r.cache_write()).sum(),
            });
        }

        let (mut shared_reads, mut other_ranges, mut shared_writes) = (None, None, None);
        let mut example_files = Vec::new();
        if file_data {
            let mut areas: HashMap<ReadArea, HashSet<usize>> = HashMap::new();
            let mut readers: HashMap<&str, HashSet<usize>> = HashMap::new();
            let mut writers: HashMap<&str, HashSet<usize>> = HashMap::new();
            for &i in &acc.streams {
                for e in &events[i] {
                    let Some(target) = e.target.as_deref() else {
                        continue;
                    };
                    match e.kind {
                        ToolKind::Read => {
                            areas.entry((target, e.range)).or_default().insert(i);
                            readers.entry(target).or_default().insert(i);
                        }
                        ToolKind::Write => {
                            writers.entry(target).or_default().insert(i);
                        }
                        _ => {}
                    }
                }
            }
            let shared_areas: Vec<&ReadArea> = areas
                .iter()
                .filter(|(_, s)| s.len() >= 2)
                .map(|(a, _)| a)
                .collect();
            let shared_files: HashSet<&str> = shared_areas.iter().map(|(p, _)| *p).collect();
            let others = readers
                .iter()
                .filter(|(p, s)| s.len() >= 2 && !shared_files.contains(**p))
                .count() as u64;
            let written: Vec<&str> = writers
                .iter()
                .filter(|(_, s)| s.len() >= 2)
                .map(|(p, _)| *p)
                .collect();
            shared_reads = Some(shared_areas.len() as u64);
            other_ranges = Some(others);
            other_range_files += others;
            shared_writes = Some(written.len() as u64);
            example_files = examples(
                written.iter().copied(),
                shared_areas.iter().map(|(p, _)| *p),
            );
        }

        let mut children = acc.children;
        children.sort_by(|a, b| {
            (std::cmp::Reverse(a.usage.input), &a.file)
                .cmp(&(std::cmp::Reverse(b.usage.input), &b.file))
        });
        roots.push(Root {
            session,
            project: acc.project,
            in_logs: acc.in_logs,
            inclusive_input: acc.direct.input + acc.delegated.input,
            direct: acc.direct,
            delegated: acc.delegated,
            agents: children.len() as u64,
            max_depth: children.iter().filter_map(|c| c.depth).max().unwrap_or(0),
            first: format_iso(times[0]),
            last: format_iso(times[times.len() - 1]),
            span_ms: span,
            active_ms: span - paused,
            paused_ms: paused,
            runs: run_count,
            agents_with_logged_run: children.iter().filter(|c| c.logged_runs > 0).count() as u64,
            logged_run_ms: children
                .iter()
                .filter_map(|c| c.logged_run_ms)
                .reduce(|a, b| a + b),
            shared_reads,
            shared_reads_other_ranges: other_ranges,
            shared_writes,
            shared_examples: example_files,
            start_context,
            children,
        });
    }
    roots.sort_by(|a, b| {
        (std::cmp::Reverse(a.inclusive_input), &a.session)
            .cmp(&(std::cmp::Reverse(b.inclusive_input), &b.session))
    });

    counts.sessions = streams
        .iter()
        .filter(|s| !s.key.1)
        .map(|s| s.id.as_str())
        .collect::<HashSet<_>>()
        .len() as u64;
    counts.sessions_with_agents = roots.len() as u64;
    counts.agents = streams.iter().filter(|s| s.key.1).count() as u64;
    counts.unassigned_agents = unassigned.len() as u64;
    counts.agents_with_logged_run = roots
        .iter()
        .flat_map(|r| &r.children)
        .chain(&unassigned)
        .filter(|c| c.logged_runs > 0)
        .count() as u64;
    counts.run_records_unlinked = dataset.agent_runs_unlinked;
    counts.nested_agents = roots
        .iter()
        .flat_map(|r| &r.children)
        .filter(|c| c.depth.is_some_and(|d| d >= 2))
        .count() as u64;
    counts.max_depth = roots.iter().map(|r| r.max_depth).max().unwrap_or(0);

    let mut counterexamples = Vec::new();
    if counts.overlaps_time_only > 0 {
        counterexamples.push(format!(
            "{} overlapping pairs have no named relation: they ran at the same time, which is \
             not a delegation",
            counts.overlaps_time_only
        ));
    }
    if counts.overlaps_delegation > 0 {
        counterexamples.push(format!(
            "{} overlapping pairs are a session and an agent it delegated to, which is expected \
             while delegating",
            counts.overlaps_delegation
        ));
    }
    if other_range_files > 0 {
        counterexamples.push(format!(
            "{other_range_files} files read by several agents were read at different lines only, \
             so no area was read twice"
        ));
    }
    let mut notes = vec![
        "Agents are linked only where the log names the parent (Claude Code: the session folder \
         of the agent's log; Codex: spawn record, fork id or the copied parent history, the \
         weakest). Project name and time never link."
            .to_owned(),
        "A run is a stretch of requests less than an hour apart; its length is first to last \
         request and still contains waiting, and a run of one request has no observed length. \
         Overlap is a reason to look, not a conflict."
            .to_owned(),
    ];
    if counts.agents_with_logged_run > 0 {
        notes.push(format!(
            "{} of {} agents have a run time in the log: the wall time of each finished run, \
             waiting included, added up per agent. Sums over agents are not elapsed time, since \
             agents can run at once. The others have none recorded or readable.",
            counts.agents_with_logged_run, counts.agents
        ));
    } else if counts.agents > 0 {
        notes.push(
            "No agent has a run time in the data read; run time stays unknown, it is not zero."
                .to_owned(),
        );
    }
    if counts.run_records_unlinked > 0 {
        notes.push(format!(
            "{} logged run times of background agents name their agent only in the notice text, \
             which is read with the tool calls: add --tool-calls to assign them.",
            counts.run_records_unlinked
        ));
    }
    if !unassigned.is_empty() {
        notes.push(format!(
            "{} agents have no usable parent chain in the logs: their requests are listed apart \
             and added to no session.",
            unassigned.len()
        ));
    }
    let copied = dataset
        .agent_links
        .iter()
        .filter(|l| l.basis == Some("copied_history"))
        .count();
    if copied > 0 {
        notes.push(format!(
            "{copied} agents have their parent from copied parent history only; the log has no \
             spawn record for them."
        ));
    }
    match tools {
        None => notes.push(
            "Tool calls were not read: add --tool-calls for files several agents read or wrote."
                .to_owned(),
        ),
        Some(t) if !t.has_targets => notes.push(
            "This source logs no file targets of tool calls, so shared files cannot be measured."
                .to_owned(),
        ),
        Some(_) => {
            notes.push(
                "A file several agents read or wrote is a reason to check, not a conflict: the \
                 parts may differ and the order is unknown."
                    .to_owned(),
            );
        }
    }
    if roots.iter().any(|r| r.start_context.is_none()) && counts.sessions_with_agents > 0 {
        notes.push(
            "The context of first requests is shown only for sessions with two or more agents \
             and recorded cache writes; it says nothing about whether delegating paid off."
                .to_owned(),
        );
    }
    if overlap_total > MAX_OVERLAPS {
        notes.push(format!(
            "{MAX_OVERLAPS} of {overlap_total} overlapping pairs are listed, those with the most \
             shared files first; the counts cover all."
        ));
    }

    SubagentReport {
        method: METHOD,
        file_data,
        counts,
        totals,
        roots,
        unassigned,
        overlaps,
        counterexamples,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolcalls::ToolStatus;

    const MIN: i64 = 60_000;

    fn req(source: usize, session: &str, sub: bool, time: i64, model: &str) -> Request {
        Request {
            session: session.into(),
            project: "p".into(),
            source,
            message_id: String::new(),
            time_ms: time,
            model: model.into(),
            sub,
            fresh: 10,
            cache_read: 100,
            cache_write_5m: 5,
            cache_write_1h: 0,
            output: 7,
            window: None,
        }
    }

    fn link(source: usize, agent: &str, parent: Option<&str>) -> AgentLink {
        AgentLink {
            source,
            agent: agent.into(),
            parent: parent.map(str::to_owned),
            basis: parent.map(|_| "spawn_record"),
        }
    }

    fn call(
        source: usize,
        session: &str,
        sub: bool,
        time: i64,
        kind: ToolKind,
        path: &str,
    ) -> ToolEvent {
        ToolEvent {
            source,
            session: session.into(),
            project: "p".into(),
            sub,
            seq: time as u64,
            time_ms: time,
            call_id: format!("c{time}{source}"),
            tool: "x".into(),
            kind,
            target: Some(path.into()),
            range: None,
            fingerprint: None,
            status: ToolStatus::Ok,
            output_bytes: None,
            has_result: true,
        }
    }

    /// Session s1 (file 0) with agent a1 (file 1, parent s1) and its own child a2 (file 2,
    /// parent a1); orphan o1 (file 3, no parent); an agent whose parent chain loops (files 4, 5);
    /// an independent session s2 (file 6) running at the same time as s1.
    fn dataset() -> Dataset {
        Dataset {
            requests: vec![
                req(0, "s1", false, 0, "m-big"),
                req(0, "s1", false, 10 * MIN, "m-big"),
                req(1, "s1", true, 2 * MIN, "m-small"),
                req(1, "s1", true, 4 * MIN, "m-small"),
                req(2, "a1", true, 3 * MIN, "m-small"),
                req(3, "x", true, 500 * MIN, ""),
                req(4, "l1", true, 600 * MIN, "m"),
                req(5, "l2", true, 700 * MIN, "m"),
                req(6, "s2", false, 3 * MIN + 1, "m-big"),
            ],
            files: (0..7).map(|i| format!("f{i}.jsonl")).collect(),
            agent_links: vec![
                link(1, "a1", Some("s1")),
                link(2, "a2", Some("a1")),
                link(3, "o1", None),
                link(4, "l1", Some("l2")),
                link(5, "l2", Some("l1")),
            ],
            ..Dataset::default()
        }
    }

    #[test]
    fn requests_are_partitioned_once_and_nested_agents_reach_their_root() {
        let d = dataset();
        let r = analyse(&d, None, None);
        let t = &r.totals;
        assert_eq!(t.requests, d.requests.len() as u64);
        assert_eq!(
            t.direct.requests + t.delegated.requests + t.unassigned.requests,
            t.requests
        );
        assert_eq!(
            t.direct.input + t.delegated.input + t.unassigned.input,
            t.input
        );
        // a1 (two requests) and a2 (one, named under a1) belong to s1.
        assert_eq!(t.delegated.requests, 3);
        assert_eq!(t.unassigned.requests, 3, "orphan and the looping chain");
        let root = &r.roots[0];
        assert_eq!(
            (root.session.as_str(), root.agents, root.max_depth),
            ("s1", 2, 2)
        );
        assert_eq!(root.direct.requests, 2);
        assert_eq!(
            root.inclusive_input,
            root.direct.input + root.delegated.input
        );
        let depths: Vec<_> = root.children.iter().map(|c| c.depth).collect();
        assert!(depths.contains(&Some(1)) && depths.contains(&Some(2)));
        assert_eq!(r.counts.nested_agents, 1);
        assert_eq!(r.counts.unassigned_agents, 3);
        assert!(r.unassigned.iter().all(|c| c.depth.is_none()));
        // Direct and delegated usage keep their model mix apart.
        assert_eq!(root.direct.models["m-big"], 2);
        assert_eq!(root.delegated.models["m-small"], 3);
    }

    #[test]
    fn logged_run_time_is_added_per_agent_and_unknown_elsewhere() {
        use crate::model::AgentRun;
        let run = |agent: &str, end_ms: i64, duration_ms: u64| AgentRun {
            agent: agent.into(),
            end_ms,
            duration_ms,
        };
        let mut d = dataset();
        d.agent_runs = vec![
            run("a1", 3 * MIN, 60_000),
            run("a1", 5 * MIN, 30_000),
            run("gone", 5 * MIN, 1_000),
        ];
        d.agent_runs_unlinked = 2;
        let r = analyse(&d, None, None);
        let root = r.roots.iter().find(|r| r.session == "s1").unwrap();
        let child = |agent: &str| {
            root.children
                .iter()
                .find(|c| c.agent.as_deref() == Some(agent))
                .unwrap()
        };
        assert_eq!(
            (child("a1").logged_runs, child("a1").logged_run_ms),
            (2, Some(90_000))
        );
        assert_eq!(
            (child("a2").logged_runs, child("a2").logged_run_ms),
            (0, None)
        );
        assert_eq!(
            (root.agents_with_logged_run, root.logged_run_ms),
            (1, Some(90_000))
        );
        assert_eq!(
            (
                r.counts.agents_with_logged_run,
                r.counts.run_records_unlinked
            ),
            (1, 2)
        );
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("add --tool-calls to assign"))
        );

        // A run that ended before the period is not counted.
        let later = analyse(&d, None, Some(4 * MIN));
        let a1 = later
            .roots
            .iter()
            .flat_map(|r| &r.children)
            .find(|c| c.agent.as_deref() == Some("a1"))
            .unwrap();
        assert_eq!((a1.logged_runs, a1.logged_run_ms), (1, Some(30_000)));
    }

    #[test]
    fn a_parent_is_never_guessed_from_project_or_time() {
        let mut d = dataset();
        d.agent_links.retain(|l| l.agent != "a1");
        let r = analyse(&d, None, None);
        // a1 lost its link although it shares project and time with s1: it is unassigned, and
        // s1 no longer has an agent. a2 still names a1, a parent that is no session in the logs.
        assert_eq!(r.roots.len(), 1);
        assert_eq!(
            (r.roots[0].session.as_str(), r.roots[0].in_logs),
            ("a1", false)
        );
        assert_eq!(r.counts.unassigned_agents, 4);
    }

    #[test]
    fn overlaps_tell_delegation_from_time_only() {
        let r = analyse(&dataset(), None, None);
        let find = |a: &str, b: &str| {
            r.overlaps
                .iter()
                .find(|o| (o.first == a && o.second == b) || (o.first == b && o.second == a))
                .map(|o| o.relation)
        };
        assert_eq!(find("s1", "a1"), Some("delegation"));
        assert_eq!(
            find("s1", "a2"),
            Some("delegation"),
            "an ancestor, not only the parent"
        );
        assert_eq!(find("a1", "a2"), Some("delegation"));
        assert_eq!(find("s1", "s2"), Some("time_only"));
        assert_eq!(find("s2", "a1"), Some("time_only"));
        assert!(
            r.counterexamples
                .iter()
                .any(|c| c.contains("not a delegation"))
        );
    }

    #[test]
    fn runs_cut_pauses_instead_of_counting_the_whole_span() {
        let mut d = dataset();
        d.requests.push(req(0, "s1", false, 5 * 60 * MIN, "m-big"));
        let root = &analyse(&d, None, None).roots[0];
        assert_eq!(root.runs, 2);
        assert!(root.paused_ms >= PAUSE_MS);
        assert_eq!(root.active_ms + root.paused_ms, root.span_ms);
        assert!(root.active_ms < root.span_ms);
    }

    #[test]
    fn shared_files_need_tool_data_and_stay_unknown_without_it() {
        let d = dataset();
        let r = analyse(&d, None, None);
        assert!(!r.file_data);
        assert!(r.overlaps.iter().all(|o| o.shared_writes.is_none()));
        assert!(r.roots[0].shared_writes.is_none());
        assert!(r.notes.iter().any(|n| n.contains("--tool-calls")));
        // A source whose log has no targets is a limit as well.
        let no_targets = ToolData::default();
        let r = analyse(&d, Some(&no_targets), None);
        assert!(r.notes.iter().any(|n| n.contains("no file targets")));
    }

    #[test]
    fn shared_reads_and_writes_are_counted_with_other_ranges_as_counterexample() {
        let mut d = dataset();
        d.requests.push(req(2, "a1", true, 4 * MIN, "m-small"));
        let mut a_read = call(1, "s1", true, 2 * MIN, ToolKind::Read, "/p/a.rs");
        a_read.range = Some((1, 50));
        let mut b_read = call(2, "a1", true, 3 * MIN, ToolKind::Read, "/p/a.rs");
        b_read.range = Some((200, 250));
        let tools = ToolData {
            events: vec![
                call(0, "s1", false, 0, ToolKind::Read, "/p/b.rs"),
                a_read,
                call(1, "s1", true, 4 * MIN, ToolKind::Write, "/p/w.rs"),
                b_read,
                call(2, "a1", true, 4 * MIN, ToolKind::Write, "/p/w.rs"),
                call(2, "a1", true, 3 * MIN, ToolKind::Read, "/p/b.rs"),
            ],
            has_targets: true,
            ..ToolData::default()
        };
        let r = analyse(&d, Some(&tools), None);
        assert!(r.file_data);
        let root = &r.roots[0];
        assert_eq!(root.shared_writes, Some(1));
        // b.rs was read by the session and by a2 at the same (whole-file) area.
        assert_eq!(root.shared_reads, Some(1));
        // a.rs was read twice at different lines: not counted as a repeat.
        assert_eq!(root.shared_reads_other_ranges, Some(1));
        assert!(root.shared_examples.contains(&"/p/w.rs".to_owned()));
        assert!(
            r.counterexamples
                .iter()
                .any(|c| c.contains("different lines"))
        );
        let pair = r
            .overlaps
            .iter()
            .find(|o| o.shared_writes == Some(1))
            .expect("agents that wrote the same file in an overlap");
        assert_eq!(pair.relation, "delegation");
    }

    #[test]
    fn start_context_needs_two_agents_and_recorded_cache_writes() {
        let mut d = dataset();
        d.agent_links[2] = link(3, "o1", Some("s1"));
        let none = analyse(&d, None, None);
        assert!(none.roots[0].start_context.is_none());
        d.stats.cache_write_measured = true;
        let some = analyse(&d, None, None);
        let sc = some.roots[0].start_context.as_ref().unwrap();
        assert_eq!(sc.agents, 3);
        assert_eq!(sc.cache_write, 15);
    }
}
