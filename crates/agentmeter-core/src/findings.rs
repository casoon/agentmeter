//! Diagnoses: rules over the usage data.
//!
//! Following `plan/referenz/00-konzept.md` section 12, a rule reports an observation and says how
//! sure it is. No M1 rule emits [`Level::Finding`]: that level is reserved for a verifiable
//! malfunction, and a size over a threshold is not one. Thresholds are constants for now, all
//! uncalibrated unless the comment says otherwise; a configuration file comes later.
//!
//! Every diagnosis carries the rule id and version, the measure with its denominator and sample,
//! evidence (log file and event id, never message text), certainty, known counterexamples and
//! one next step.

use crate::format::{count, tokens};
#[cfg(test)]
use crate::model::ReadStats;
use crate::model::{Compaction, Dataset, Request, SessionStart};
use crate::time::{HOUR_MS, MINUTE_MS};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};

const BASELINE_VERSION: u32 = 1;
const REBUILD_VERSION: u32 = 1;
const THRASH_VERSION: u32 = 1;
const BIG_CONTEXT_VERSION: u32 = 2;
const LONG_SESSION_VERSION: u32 = 1;

/// Median initial context above which `baseline-load` reports.
/// Source: `plan/referenz/05` (30k) and the public benchmark traces in section 12 (17k with
/// minimal configuration). Uncalibrated assumption.
const BASELINE_NOTICE_TOKENS: u64 = 30_000;
/// Clean session starts needed before a median means anything. Assumption.
const BASELINE_MIN_SAMPLE: usize = 5;

/// Smallest cache write after a pause that counts as a rebuild.
/// Source: `plan/referenz/00-konzept.md` section 6 (`cache-rebuild`, 100k).
const REBUILD_MIN_WRITE_TOKENS: u64 = 100_000;
/// Shortest cache lifetime; a gap below it cannot have expired any cache.
/// Source: Anthropic prompt caching, default lifetime 5 minutes (`plan/referenz/04`).
const CACHE_TTL_SHORT_MS: i64 = 5 * MINUTE_MS;
/// Longest cache lifetime; a gap above it has expired every cache.
/// Source: same, extended lifetime 1 hour.
const CACHE_TTL_LONG_MS: i64 = HOUR_MS;
/// Unexpected rebuilds from which the rule is a notice instead of info. Uncalibrated assumption.
const REBUILD_NOTICE_COUNT: u64 = 10;

/// Growth of the context between two automatic compactions below which they are suspicious.
/// Growth means `preTokens` of the later minus `postTokens` of the earlier compaction. Neither
/// the unknown effective window nor the amount of new content is considered, hence only a
/// suspicion. Source: `plan/referenz/00-konzept.md` section 11, value uncalibrated.
const THRASH_MAX_GROWTH_TOKENS: u64 = 100_000;

/// Context size above which a request counts as big.
/// Source: `plan/referenz/00-konzept.md` section 6 (`big-context`, 500k).
const BIG_CONTEXT_TOKENS: u64 = 500_000;
/// Where the log records the context window (Codex), a request is big above this share of its
/// own window. Uncalibrated assumption: half the window is where compaction pressure starts.
const BIG_CONTEXT_WINDOW_SHARE_PCT: u64 = 50;
/// Share of all tokens from big requests from which the rule speaks. Uncalibrated assumption.
const BIG_CONTEXT_MIN_SHARE_PCT: f64 = 10.0;

/// Session length from which `long-session` reports.
/// Source: `plan/referenz/00-konzept.md` section 6 (`long-session`, 24 h).
const LONG_SESSION_MS: i64 = 24 * HOUR_MS;

/// Evidence entries listed per diagnosis.
const EVIDENCE_MAX: usize = 3;

/// Most severe first. `Finding` is reserved for a verifiable malfunction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Finding,
    Notice,
    Info,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Finding => "finding",
            Level::Notice => "notice",
            Level::Info => "info",
        }
    }
}

/// A number with what it was measured against.
#[derive(Debug, Clone, Serialize)]
pub struct Measure {
    pub name: &'static str,
    pub value: f64,
    pub unit: &'static str,
    pub denominator: u64,
    pub denominator_unit: &'static str,
    pub sample: u64,
    pub sample_unit: &'static str,
}

/// Where to look: a log file (relative to the projects directory) and an event id.
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub file: String,
    pub event: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule: &'static str,
    pub rule_version: u32,
    pub level: Level,
    /// `observed` (counted in the log), `likely` or `suspected` (inferred).
    pub certainty: &'static str,
    pub message: String,
    pub measure: Measure,
    pub evidence: Vec<Evidence>,
    pub counterexamples: Vec<&'static str>,
    pub next_step: String,
    /// More numbers behind the message, with stable keys.
    pub values: Map<String, Value>,
}

/// What happened to a rule: it reported, found nothing, or could not be measured.
#[derive(Debug, Clone, Serialize)]
pub struct RuleStatus {
    pub rule: &'static str,
    pub rule_version: u32,
    /// `observed` (a diagnosis was reported), `checked_none` (measured, below the threshold) or
    /// `not_measurable`.
    pub status: &'static str,
    /// Why the rule could not be measured.
    pub reason: Option<String>,
    /// What was measured, for `observed` and `checked_none`.
    pub message: Option<String>,
    pub measure: Option<Measure>,
}

#[derive(Debug, Serialize)]
pub struct Evaluation {
    /// `no_data` (nothing to analyse), `findings` or `none`.
    pub verdict: &'static str,
    pub findings: Vec<Finding>,
    pub rules: Vec<RuleStatus>,
}

/// Result of one rule.
enum Outcome {
    Reported(Box<Finding>),
    /// Checked, nothing to report: what was measured, so that "nothing" can be told from "not looked".
    Checked(String, Measure),
    NotMeasurable(String),
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Nearest-rank quantile of a sorted, non-empty slice.
fn quantile(sorted: &[u64], p: f64) -> u64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn values(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

fn request_evidence(dataset: &Dataset, requests: &[&Request]) -> Vec<Evidence> {
    requests
        .iter()
        .take(EVIDENCE_MAX)
        .map(|r| Evidence {
            file: dataset.files.get(r.source).cloned().unwrap_or_default(),
            event: r.message_id.clone(),
        })
        .collect()
}

/// Evaluates all rules over the dataset. `since_ms` limits what is reported, while the earlier
/// data still tells where a session started and what happened before a pause.
pub fn evaluate(dataset: &Dataset, since_ms: Option<i64>) -> Evaluation {
    let in_window = |time_ms: i64| since_ms.is_none_or(|since| time_ms >= since);
    let window: Vec<&Request> = dataset
        .requests
        .iter()
        .filter(|r| in_window(r.time_ms))
        .collect();
    let mut sorted: Vec<&Request> = dataset.requests.iter().collect();
    sorted.sort_by_key(|r| r.time_ms);

    let outcomes = [
        (
            "baseline-load",
            BASELINE_VERSION,
            baseline_load(dataset, &sorted, in_window),
        ),
        (
            "cache-rebuild",
            REBUILD_VERSION,
            cache_rebuild(dataset, &sorted, in_window),
        ),
        (
            "compact-thrash",
            THRASH_VERSION,
            compact_thrash(dataset, &sorted, in_window),
        ),
        (
            "big-context",
            BIG_CONTEXT_VERSION,
            big_context(dataset, &window),
        ),
        (
            "long-session",
            LONG_SESSION_VERSION,
            long_session(dataset, &window),
        ),
    ];
    let mut findings = Vec::new();
    let mut rules = Vec::new();
    for (rule, rule_version, outcome) in outcomes {
        let (status, reason, message, measure) = match outcome {
            _ if window.is_empty() => (
                "not_measurable",
                Some("no requests in the selected period".to_owned()),
                None,
                None,
            ),
            Outcome::Reported(finding) => {
                let shown = (Some(finding.message.clone()), Some(finding.measure.clone()));
                findings.push(*finding);
                ("observed", None, shown.0, shown.1)
            }
            Outcome::Checked(message, measure) => {
                ("checked_none", None, Some(message), Some(measure))
            }
            Outcome::NotMeasurable(reason) => ("not_measurable", Some(reason), None, None),
        };
        rules.push(RuleStatus {
            rule,
            rule_version,
            status,
            reason,
            message,
            measure,
        });
    }
    findings.sort_by_key(|f| f.level);
    let verdict = if window.is_empty() {
        "no_data"
    } else if findings.is_empty() {
        "none"
    } else {
        "findings"
    };
    Evaluation {
        verdict,
        findings,
        rules,
    }
}

/// Context of the first request of each session. Without a breakdown by component this is only
/// the initial context: it also holds the user's first prompt, attachments and inherited
/// history, so no configuration cause is claimed.
fn baseline_load(
    dataset: &Dataset,
    sorted: &[&Request],
    in_window: impl Fn(i64) -> bool,
) -> Outcome {
    let starts: HashMap<&str, &SessionStart> = dataset
        .session_starts
        .iter()
        .map(|s| (s.session.as_str(), s))
        .collect();
    let mut first: HashMap<&str, &Request> = HashMap::new();
    for r in sorted.iter().filter(|r| !r.sub) {
        first.entry(r.session.as_str()).or_insert(r);
    }
    let mut clean: Vec<&Request> = Vec::new();
    let (mut inherited, mut no_start) = (0u64, 0u64);
    for (session, request) in &first {
        let Some(start) = starts.get(session).filter(|_| in_window(request.time_ms)) else {
            continue;
        };
        match (start.clean, start.reason) {
            (true, _) => clean.push(request),
            (false, Some("inherited-history")) => inherited += 1,
            (false, _) => no_start += 1,
        }
    }
    clean.sort_by_key(|r| (std::cmp::Reverse(r.context()), r.time_ms));
    let all_seen = clean.len() as u64 + inherited + no_start;
    if all_seen == 0 {
        return Outcome::NotMeasurable("no session start in the selected period".to_owned());
    }
    if clean.len() < BASELINE_MIN_SAMPLE {
        return Outcome::NotMeasurable(format!(
            "only {} clean session starts ({} needed); {inherited} resumed or forked, {no_start} without log start",
            clean.len(),
            BASELINE_MIN_SAMPLE
        ));
    }
    let mut loads: Vec<u64> = clean.iter().map(|r| r.context()).collect();
    loads.sort_unstable();
    let median = quantile(&loads, 0.5);
    let (p25, p75) = (quantile(&loads, 0.25), quantile(&loads, 0.75));
    if median <= BASELINE_NOTICE_TOKENS {
        return Outcome::Checked(
            format!(
                "median initial context {} of {} clean session starts (quartiles {} / {}), not above {}",
                tokens(median),
                count(clean.len() as u64),
                tokens(p25),
                tokens(p75),
                tokens(BASELINE_NOTICE_TOKENS)
            ),
            Measure {
                name: "median initial context",
                value: median as f64,
                unit: "tokens",
                denominator: all_seen,
                denominator_unit: "session starts in the period",
                sample: clean.len() as u64,
                sample_unit: "clean session starts",
            },
        );
    }
    Outcome::Reported(Box::new(Finding {
        rule: "baseline-load",
        rule_version: BASELINE_VERSION,
        level: Level::Notice,
        certainty: "observed",
        message: format!(
            "initial context of {} clean session starts: median {} (quartiles {} / {}); not counted: {} resumed or forked, {} without log start",
            count(clean.len() as u64),
            tokens(median),
            tokens(p25),
            tokens(p75),
            count(inherited),
            count(no_start)
        ),
        measure: Measure {
            name: "median initial context",
            value: median as f64,
            unit: "tokens",
            denominator: all_seen,
            denominator_unit: "session starts in the period",
            sample: clean.len() as u64,
            sample_unit: "clean session starts",
        },
        evidence: request_evidence(dataset, &clean),
        counterexamples: vec![
            "the first request also contains your first prompt and attachments",
            "a session started from another directory or with a large first task starts larger",
        ],
        next_step: "open a fresh session in an empty directory and compare its first request; the agent's own context view (for example /context in Claude Code) lists the components".into(),
        values: values(vec![
            ("p25_tokens", json!(p25)),
            ("p75_tokens", json!(p75)),
            ("excluded_inherited_history", json!(inherited)),
            ("excluded_no_log_start", json!(no_start)),
            ("threshold_tokens", json!(BASELINE_NOTICE_TOKENS)),
        ]),
    }))
}

/// A large cache write after a pause is an observation. The lifetime of the cache that was
/// written *now* says nothing about the one that was lost, so the cause comes only from earlier
/// writes in the same file and from the length of the pause.
fn cache_rebuild(
    dataset: &Dataset,
    sorted: &[&Request],
    in_window: impl Fn(i64) -> bool,
) -> Outcome {
    if !dataset.stats.cache_write_measured {
        return Outcome::NotMeasurable("no cache write data in this source".to_owned());
    }
    let mut streams: BTreeMap<usize, Vec<&Request>> = BTreeMap::new();
    for r in sorted {
        streams.entry(r.source).or_default().push(r);
    }
    let mut compacted: HashMap<&str, Vec<i64>> = HashMap::new();
    for c in &dataset.compactions {
        compacted
            .entry(c.session.as_str())
            .or_default()
            .push(c.time_ms);
    }

    #[derive(Default)]
    struct Tally {
        pauses: u64,
        rebuilds: u64,
        rewritten: u64,
        expired: u64,
        likely_expired_5m: u64,
        unknown: u64,
        expected: u64,
    }
    let mut t = Tally::default();
    let mut events: Vec<&Request> = Vec::new();
    let mut expected_events: Vec<&Request> = Vec::new();
    for stream in streams.values() {
        let (mut seen_5m, mut seen_1h) = (false, false);
        for pair in stream.windows(2) {
            let (prev, cur) = (pair[0], pair[1]);
            seen_5m |= prev.cache_write_5m > 0;
            seen_1h |= prev.cache_write_1h > 0;
            let gap = cur.time_ms - prev.time_ms;
            if gap <= CACHE_TTL_SHORT_MS || !in_window(cur.time_ms) {
                continue;
            }
            t.pauses += 1;
            if cur.cache_write() < REBUILD_MIN_WRITE_TOKENS {
                continue;
            }
            t.rebuilds += 1;
            t.rewritten += cur.cache_write();
            let expected = prev.model != cur.model
                || compacted.get(cur.session.as_str()).is_some_and(|times| {
                    times.iter().any(|c| *c > prev.time_ms && *c <= cur.time_ms)
                });
            if expected {
                t.expected += 1;
                expected_events.push(cur);
                continue;
            }
            events.push(cur);
            if gap > CACHE_TTL_LONG_MS {
                t.expired += 1;
            } else if seen_5m && !seen_1h {
                t.likely_expired_5m += 1;
            } else {
                t.unknown += 1;
            }
        }
    }
    if t.rebuilds == 0 {
        return Outcome::Checked(
            format!(
                "0 cache writes of {}+ after {} pauses over 5 min in {} log files",
                tokens(REBUILD_MIN_WRITE_TOKENS),
                count(t.pauses),
                count(streams.len() as u64)
            ),
            Measure {
                name: "pauses over 5 min followed by a cache write of 100k+",
                value: 0.0,
                unit: "rebuilds",
                denominator: t.pauses,
                denominator_unit: "pauses over 5 min",
                sample: streams.len() as u64,
                sample_unit: "log files",
            },
        );
    }
    let unexpected = t.rebuilds - t.expected;
    // Unexpected events first: they are the ones worth looking at.
    events.extend(expected_events);
    Outcome::Reported(Box::new(Finding {
        rule: "cache-rebuild",
        rule_version: REBUILD_VERSION,
        level: if unexpected >= REBUILD_NOTICE_COUNT { Level::Notice } else { Level::Info },
        certainty: "observed",
        message: format!(
            "{} of {} pauses over 5 min were followed by a cache write of {}+ ({} tokens); of these {} expected (model change or compaction), {} gap over 1 h, {} 5 min cache likely expired, {} cause unknown",
            count(t.rebuilds),
            count(t.pauses),
            tokens(REBUILD_MIN_WRITE_TOKENS),
            tokens(t.rewritten),
            count(t.expected),
            count(t.expired),
            count(t.likely_expired_5m),
            count(t.unknown)
        ),
        measure: Measure {
            name: "pauses over 5 min followed by a cache write of 100k+",
            value: t.rebuilds as f64,
            unit: "rebuilds",
            denominator: t.pauses,
            denominator_unit: "pauses over 5 min",
            sample: streams.len() as u64,
            sample_unit: "log files",
        },
        evidence: request_evidence(dataset, &events),
        counterexamples: vec![
            "a pause is part of the work; the rebuild after it is expected, not avoidable by itself",
            "an edited memory file, a changed tool list or a split cache prefix also rewrites the cache",
            "the cache lifetime of the earlier writes is only known if the file shows them",
        ],
        next_step: "check in the evidence files whether the pause matches your work; if 5 min pauses dominate, the 1 h cache (promptCacheTtl) may reduce rewrites, but its writes cost more, so compare before switching".into(),
        values: values(vec![
            ("rewritten_tokens", json!(t.rewritten)),
            ("expected", json!(t.expected)),
            ("cause_gap_over_1h", json!(t.expired)),
            ("cause_5m_cache_likely_expired", json!(t.likely_expired_5m)),
            ("cause_unknown", json!(t.unknown)),
        ]),
    }))
}

/// Two automatic compactions with little context growth in between. The growth is the only
/// measure; request count and time are reported with it. Only a suspicion.
fn compact_thrash(
    dataset: &Dataset,
    sorted: &[&Request],
    in_window: impl Fn(i64) -> bool,
) -> Outcome {
    let mut by_session: BTreeMap<&str, Vec<&Compaction>> = BTreeMap::new();
    for c in &dataset.compactions {
        by_session.entry(c.session.as_str()).or_default().push(c);
    }
    let (mut examined, mut unmeasurable, mut manual) = (0u64, 0u64, 0u64);
    // (growth, requests between, minutes between, later compaction)
    let mut hits: Vec<(u64, u64, i64, &Compaction)> = Vec::new();
    for list in by_session.values_mut() {
        list.sort_by_key(|c| c.time_ms);
        for pair in list.windows(2) {
            if !in_window(pair[1].time_ms) {
                continue;
            }
            if pair[1].auto == Some(false) {
                manual += 1;
                continue;
            }
            // An unknown trigger or a missing size after compaction cannot be measured.
            let (true, Some(after)) = (pair[1].auto.is_some(), pair[0].tokens_after) else {
                unmeasurable += 1;
                continue;
            };
            examined += 1;
            let growth = pair[1].tokens_before.saturating_sub(after);
            if growth < THRASH_MAX_GROWTH_TOKENS {
                let between = sorted
                    .iter()
                    .filter(|r| {
                        !r.sub
                            && r.session == pair[0].session
                            && r.time_ms > pair[0].time_ms
                            && r.time_ms <= pair[1].time_ms
                    })
                    .count() as u64;
                hits.push((
                    growth,
                    between,
                    (pair[1].time_ms - pair[0].time_ms) / MINUTE_MS,
                    pair[1],
                ));
            }
        }
    }
    if examined == 0 && unmeasurable > 0 {
        return Outcome::NotMeasurable(format!(
            "{unmeasurable} compaction pairs lack the size after compaction or the trigger"
        ));
    }
    hits.sort_by_key(|h| h.0);
    let Some(smallest) = hits.first() else {
        let message = if examined == 0 {
            "no automatic compaction pair in the period".to_owned()
        } else {
            format!(
                "0 of {} automatic compaction pairs came with under {} of context growth",
                count(examined),
                tokens(THRASH_MAX_GROWTH_TOKENS)
            )
        };
        return Outcome::Checked(
            message,
            Measure {
                name: "automatic compaction pairs with small context growth",
                value: 0.0,
                unit: "pairs",
                denominator: examined,
                denominator_unit: "automatic compaction pairs with known size after compaction",
                sample: examined,
                sample_unit: "compaction pairs",
            },
        );
    };
    Outcome::Reported(Box::new(Finding {
        rule: "compact-thrash",
        rule_version: THRASH_VERSION,
        level: Level::Notice,
        certainty: "suspected",
        message: format!(
            "{} of {} automatic compaction pairs came with under {} of context growth in between (smallest {}: {} requests, {} min)",
            count(hits.len() as u64),
            count(examined),
            tokens(THRASH_MAX_GROWTH_TOKENS),
            tokens(smallest.0),
            count(smallest.1),
            smallest.2
        ),
        measure: Measure {
            name: "automatic compaction pairs with small context growth",
            value: hits.len() as f64,
            unit: "pairs",
            denominator: examined,
            denominator_unit: "automatic compaction pairs with known size after compaction",
            sample: examined,
            sample_unit: "compaction pairs",
        },
        evidence: hits
            .iter()
            .take(EVIDENCE_MAX)
            .map(|h| Evidence {
                file: dataset.files.get(h.3.source).cloned().unwrap_or_default(),
                event: h.3.event_id.clone(),
            })
            .collect(),
        counterexamples: vec![
            "growth is the context size before minus after, not new content",
            "a small effective window (not visible in the log) makes frequent compaction expected",
            "manual compactions are excluded; the user chose them",
        ],
        next_step: "look at the evidence sessions: if each compaction followed a large file or tool result, the window is the limit, not the habit".into(),
        values: values(vec![
            ("pairs_unmeasurable_no_post_tokens", json!(unmeasurable)),
            ("pairs_second_manual_excluded", json!(manual)),
            ("smallest_growth_tokens", json!(smallest.0)),
            ("threshold_tokens", json!(THRASH_MAX_GROWTH_TOKENS)),
        ]),
    }))
}

/// Token share of big requests, shown with their cache share. A request is big when its context
/// exceeds [`BIG_CONTEXT_WINDOW_SHARE_PCT`] of its own context window if the log records one
/// (Codex), else [`BIG_CONTEXT_TOKENS`] (Claude Code logs no window).
fn big_context(dataset: &Dataset, window: &[&Request]) -> Outcome {
    let limit = |r: &Request| {
        r.window.map_or(BIG_CONTEXT_TOKENS, |w| {
            w * BIG_CONTEXT_WINDOW_SHARE_PCT / 100
        })
    };
    let windowed = window.iter().any(|r| r.window.is_some());
    let (what, measure_name) = if windowed {
        (
            format!("{BIG_CONTEXT_WINDOW_SHARE_PCT} % of their context window"),
            "share of tokens from requests above 50 % of their context window",
        )
    } else {
        (
            tokens(BIG_CONTEXT_TOKENS),
            "share of tokens from requests above 500k",
        )
    };
    let total: u64 = window.iter().map(|r| r.context()).sum();
    let mut big: Vec<&Request> = window
        .iter()
        .copied()
        .filter(|r| r.context() > limit(r))
        .collect();
    let big_total: u64 = big.iter().map(|r| r.context()).sum();
    let share = percent(big_total, total);
    let largest_context = window.iter().map(|r| r.context()).max().unwrap_or(0);
    let largest_window = window.iter().filter_map(|r| r.window).max();
    let seen = match largest_window {
        Some(w) => format!(
            "largest context {}, largest window {}",
            tokens(largest_context),
            tokens(w)
        ),
        None => format!("largest context {}", tokens(largest_context)),
    };
    let of_requests = percent(big.len() as u64, window.len() as u64);
    let measure = Measure {
        name: measure_name,
        value: round1(share),
        unit: "percent",
        denominator: total,
        denominator_unit: "tokens in the period",
        sample: big.len() as u64,
        sample_unit: "big requests",
    };
    if share < BIG_CONTEXT_MIN_SHARE_PCT {
        return Outcome::Checked(
            format!(
                "{} of {} requests above {what} ({share:.1} % of tokens, below {BIG_CONTEXT_MIN_SHARE_PCT:.0} %); {seen}",
                count(big.len() as u64),
                count(window.len() as u64)
            ),
            Measure {
                sample: window.len() as u64,
                sample_unit: "requests",
                ..measure
            },
        );
    }
    big.sort_by_key(|r| std::cmp::Reverse(r.context()));
    let cache = percent(big.iter().map(|r| r.cache_read).sum(), big_total);
    Outcome::Reported(Box::new(Finding {
        rule: "big-context",
        rule_version: BIG_CONTEXT_VERSION,
        level: Level::Notice,
        certainty: "observed",
        message: format!(
            "{share:.0} % of tokens come from requests above {what} ({of_requests:.1} % of requests); cache read share there {cache:.0} %; {seen}"
        ),
        measure,
        evidence: request_evidence(dataset, &big),
        counterexamples: vec![
            "a high cache read share makes a large context cheap per token",
            "the task may need the context; quality loss at length is not measurable in the log",
        ],
        next_step: "list the sessions (--sessions) to see which hold these contexts and whether they were one task".into(),
        values: values(vec![
            ("threshold_tokens", json!((!windowed).then_some(BIG_CONTEXT_TOKENS))),
            ("threshold_window_share_pct", json!(windowed.then_some(BIG_CONTEXT_WINDOW_SHARE_PCT))),
            ("largest_context_tokens", json!(largest_context)),
            ("largest_window_tokens", json!(largest_window)),
            ("share_of_requests_pct", json!(round1(of_requests))),
            ("cache_read_pct", json!(round1(cache))),
        ]),
    }))
}

/// Sessions whose first and last request are more than [`LONG_SESSION_MS`] apart.
fn long_session(dataset: &Dataset, window: &[&Request]) -> Outcome {
    // session -> (first, last, requests, peak context, a request of the peak)
    let mut spans: HashMap<&str, (i64, i64, u64, u64, &Request)> = HashMap::new();
    for r in window.iter().filter(|r| !r.sub) {
        let span = spans
            .entry(r.session.as_str())
            .or_insert((r.time_ms, r.time_ms, 0, 0, r));
        span.0 = span.0.min(r.time_ms);
        span.1 = span.1.max(r.time_ms);
        span.2 += 1;
        if r.context() > span.3 {
            span.3 = r.context();
            span.4 = r;
        }
    }
    let total = spans.len() as u64;
    let mut long: Vec<_> = spans
        .values()
        .filter(|(first, last, ..)| last - first > LONG_SESSION_MS)
        .collect();
    long.sort_by_key(|(first, last, ..)| std::cmp::Reverse(last - first));
    let Some(longest) = long.first() else {
        let longest_span = spans
            .values()
            .map(|(first, last, ..)| last - first)
            .max()
            .unwrap_or(0);
        return Outcome::Checked(
            format!(
                "0 of {} sessions span more than 24 h (longest {:.1} h)",
                count(total),
                longest_span as f64 / HOUR_MS as f64
            ),
            Measure {
                name: "sessions spanning more than 24 h",
                value: 0.0,
                unit: "sessions",
                denominator: total,
                denominator_unit: "sessions in the period",
                sample: total,
                sample_unit: "sessions",
            },
        );
    };
    let hours = (longest.1 - longest.0) as f64 / HOUR_MS as f64;
    let evidence: Vec<&Request> = long.iter().map(|l| l.4).collect();
    Outcome::Reported(Box::new(Finding {
        rule: "long-session",
        rule_version: LONG_SESSION_VERSION,
        level: Level::Info,
        certainty: "observed",
        message: format!(
            "{} of {} sessions span more than 24 h (longest {:.0} h, {} requests, peak context {})",
            count(long.len() as u64),
            count(total),
            hours,
            count(longest.2),
            tokens(longest.3)
        ),
        measure: Measure {
            name: "sessions spanning more than 24 h",
            value: long.len() as f64,
            unit: "sessions",
            denominator: total,
            denominator_unit: "sessions in the period",
            sample: total,
            sample_unit: "sessions",
        },
        evidence: request_evidence(dataset, &evidence),
        counterexamples: vec![
            "wall-clock span includes idle time, nights and weekends",
            "a long session can be one task with a warm cache; many short sessions can cost more",
        ],
        next_step: "check the span against your work: if the topic changed within the session, a new session starts with a smaller context".into(),
        values: values(vec![
            ("longest_hours", json!(round1(hours))),
            ("longest_requests", json!(longest.2)),
            ("longest_peak_context_tokens", json!(longest.3)),
            ("threshold_hours", json!(24)),
        ]),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(
        session: &str,
        source: usize,
        minute: i64,
        read: u64,
        write_5m: u64,
        write_1h: u64,
    ) -> Request {
        Request {
            session: session.into(),
            project: "demo".into(),
            source,
            message_id: format!("msg_{session}_{minute}"),
            time_ms: minute * MINUTE_MS,
            model: "m".into(),
            sub: false,
            fresh: 1,
            cache_read: read,
            cache_write_5m: write_5m,
            cache_write_1h: write_1h,
            output: 1,
            window: None,
        }
    }

    fn compaction(
        session: &str,
        minute: i64,
        before: u64,
        after: Option<u64>,
        auto: Option<bool>,
    ) -> Compaction {
        Compaction {
            session: session.into(),
            project: "demo".into(),
            source: 0,
            event_id: format!("uuid-{minute}"),
            time_ms: minute * MINUTE_MS,
            tokens_before: before,
            tokens_after: after,
            auto,
        }
    }

    fn dataset(requests: Vec<Request>, compactions: Vec<Compaction>) -> Dataset {
        Dataset {
            requests,
            compactions,
            files: vec!["proj/a.jsonl".into()],
            stats: ReadStats {
                cache_write_measured: true,
                ..ReadStats::default()
            },
            ..Dataset::default()
        }
    }

    fn start(session: &str, clean: bool) -> SessionStart {
        SessionStart {
            session: session.into(),
            source: 0,
            clean,
            reason: if clean {
                None
            } else {
                Some("inherited-history")
            },
        }
    }

    fn find<'a>(e: &'a Evaluation, id: &str) -> Option<&'a Finding> {
        e.findings.iter().find(|f| f.rule == id)
    }

    fn status<'a>(e: &'a Evaluation, id: &str) -> &'a RuleStatus {
        e.rules.iter().find(|r| r.rule == id).unwrap()
    }

    #[test]
    fn no_rule_emits_a_finding_level() {
        let d = dataset(vec![req("a", 0, 0, 600_000, 0, 0)], vec![]);
        assert!(
            evaluate(&d, None)
                .findings
                .iter()
                .all(|f| f.level != Level::Finding)
        );
    }

    #[test]
    fn empty_data_is_not_the_same_as_no_findings() {
        let e = evaluate(&Dataset::default(), None);
        assert_eq!(e.verdict, "no_data");
        assert!(e.rules.iter().all(|r| r.status == "not_measurable"));
        let d = dataset(vec![req("a", 0, 0, 10, 0, 0)], vec![]);
        assert_eq!(evaluate(&d, None).verdict, "none");
    }

    #[test]
    fn rebuild_cause_comes_from_earlier_writes_not_the_new_one() {
        // Earlier 5 min write, 10 min pause: likely expired. The new write's lifetime is ignored.
        let d = dataset(
            vec![req("a", 0, 0, 0, 10, 0), req("a", 0, 10, 0, 0, 150_000)],
            vec![],
        );
        let e = evaluate(&d, None);
        assert_eq!(
            find(&e, "cache-rebuild").unwrap().values["cause_5m_cache_likely_expired"],
            1
        );
        // Earlier 1 h write, 10 min pause: should still have been alive, cause unknown.
        let d = dataset(
            vec![req("a", 0, 0, 0, 0, 10), req("a", 0, 10, 0, 150_000, 0)],
            vec![],
        );
        assert_eq!(
            find(&evaluate(&d, None), "cache-rebuild").unwrap().values["cause_unknown"],
            1
        );
        // No earlier write at all: unknown.
        let d = dataset(
            vec![req("a", 0, 0, 10, 0, 0), req("a", 0, 10, 0, 150_000, 0)],
            vec![],
        );
        assert_eq!(
            find(&evaluate(&d, None), "cache-rebuild").unwrap().values["cause_unknown"],
            1
        );
        // Gap over an hour expires every cache.
        let d = dataset(
            vec![req("a", 0, 0, 10, 0, 0), req("a", 0, 90, 0, 150_000, 0)],
            vec![],
        );
        assert_eq!(
            find(&evaluate(&d, None), "cache-rebuild").unwrap().values["cause_gap_over_1h"],
            1
        );
    }

    #[test]
    fn rebuild_after_compaction_is_counted_as_expected() {
        let d = dataset(
            vec![req("a", 0, 0, 0, 10, 0), req("a", 0, 10, 0, 150_000, 0)],
            vec![compaction("a", 5, 900_000, Some(20_000), Some(true))],
        );
        let e = evaluate(&d, None);
        let f = find(&e, "cache-rebuild").unwrap();
        assert_eq!(f.values["expected"], 1);
        assert_eq!(f.level, Level::Info);
    }

    #[test]
    fn thrash_is_a_suspicion_with_measured_growth() {
        let d = dataset(
            vec![req("a", 0, 1, 1, 0, 0)],
            vec![
                compaction("a", 0, 900_000, Some(20_000), Some(true)),
                compaction("a", 10, 90_000, Some(20_000), Some(true)),
            ],
        );
        let e = evaluate(&d, None);
        let f = find(&e, "compact-thrash").unwrap();
        assert_eq!(f.values["smallest_growth_tokens"], 70_000);
        assert_eq!(f.certainty, "suspected");
        assert_eq!(f.measure.denominator, 1);
    }

    #[test]
    fn thrash_skips_manual_and_reports_missing_post_tokens() {
        let manual = dataset(
            vec![req("a", 0, 1, 1, 0, 0)],
            vec![
                compaction("a", 0, 900_000, Some(20_000), Some(true)),
                compaction("a", 10, 90_000, Some(20_000), Some(false)),
            ],
        );
        assert!(find(&evaluate(&manual, None), "compact-thrash").is_none());
        let blind = dataset(
            vec![req("a", 0, 1, 1, 0, 0)],
            vec![
                compaction("a", 0, 900_000, None, Some(true)),
                compaction("a", 10, 90_000, None, Some(true)),
            ],
        );
        assert_eq!(
            status(&evaluate(&blind, None), "compact-thrash").status,
            "not_measurable"
        );
    }

    #[test]
    fn big_context_stays_a_notice_even_with_low_cache_share() {
        let d = dataset(vec![req("a", 0, 0, 0, 600_000, 0)], vec![]);
        let e = evaluate(&d, None);
        let f = find(&e, "big-context").unwrap();
        assert_eq!(f.level, Level::Notice);
        assert_eq!(f.values["cache_read_pct"], 0.0);
    }

    fn five_starts(clean_first: bool) -> Dataset {
        let mut d = dataset(vec![], vec![]);
        for i in 0..6 {
            let s = format!("s{i}");
            d.requests.push(req(&s, 0, i, 0, 70_000, 0));
            d.session_starts.push(start(&s, clean_first || i > 0));
        }
        d
    }

    #[test]
    fn baseline_excludes_resumed_sessions_and_claims_no_cause() {
        let d = five_starts(false);
        // One of six starts inherited history; five clean remain.
        let e = evaluate(&d, None);
        let f = find(&e, "baseline-load").unwrap();
        assert_eq!(f.measure.sample, 5);
        assert_eq!(f.values["excluded_inherited_history"], 1);
        assert_eq!(f.level, Level::Notice);
        assert!(!f.message.contains("CLAUDE.md"));
    }

    #[test]
    fn baseline_with_too_few_clean_starts_is_not_measurable() {
        let mut d = five_starts(true);
        d.session_starts
            .iter_mut()
            .take(3)
            .for_each(|s| *s = start(&s.session, false));
        assert_eq!(
            status(&evaluate(&d, None), "baseline-load").status,
            "not_measurable"
        );
    }

    #[test]
    fn baseline_ignores_sessions_started_before_the_window() {
        let d = five_starts(true);
        let e = evaluate(&d, Some(100 * MINUTE_MS));
        assert_eq!(e.verdict, "no_data");
    }

    #[test]
    fn long_session_is_info() {
        let d = dataset(
            vec![req("a", 0, 0, 0, 1, 0), req("a", 0, 25 * 60, 1, 0, 0)],
            vec![],
        );
        assert_eq!(
            find(&evaluate(&d, None), "long-session").unwrap().level,
            Level::Info
        );
    }

    #[test]
    fn every_rule_has_exactly_one_status_with_its_measure() {
        let d = dataset(vec![req("a", 0, 0, 10, 0, 0)], vec![]);
        let e = evaluate(&d, None);
        assert_eq!(e.rules.len(), 5);
        for r in &e.rules {
            match r.status {
                "checked_none" => assert!(r.message.is_some() && r.measure.is_some(), "{}", r.rule),
                "not_measurable" => assert!(r.reason.is_some(), "{}", r.rule),
                "observed" => assert!(r.measure.is_some()),
                other => panic!("unexpected status {other}"),
            }
        }
        assert_eq!(status(&e, "big-context").status, "checked_none");
        assert!(
            status(&e, "big-context")
                .message
                .as_deref()
                .unwrap()
                .contains("0 of 1 requests above 500k")
        );
    }

    #[test]
    fn big_context_uses_the_logged_window_when_there_is_one() {
        let mut big = req("a", 0, 0, 150_000, 0, 0);
        big.window = Some(258_400);
        let e = evaluate(&dataset(vec![big.clone()], vec![]), None);
        let f = find(&e, "big-context").unwrap();
        assert!(f.message.contains("50 % of their context window"));
        assert!(f.message.contains("largest window"));
        big.window = Some(475_000);
        let e = evaluate(&dataset(vec![big], vec![]), None);
        let r = status(&e, "big-context");
        assert_eq!(r.status, "checked_none");
        assert!(
            r.message
                .as_deref()
                .unwrap()
                .contains("largest window 475k")
        );
    }
}
