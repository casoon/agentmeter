//! `--compare`: two periods of one source side by side, computed from the same logs.
//!
//! The interface other features build on (before/after capture, gaps in a period) is
//! [`compare`]: two [`Window`]s in, one [`Comparison`] out, no I/O and no wall clock. Both
//! windows go through the existing analysis, [`summarize`] for totals and [`evaluate`] for
//! `baseline-load` and `cache-rebuild`; there is no second calculation. The windows are
//! half-open (`from_ms` included, `to_ms` excluded) and may be any two ranges, adjacent or not.
//!
//! The result lists every [`Metric`] with both values, absolute and relative change and the
//! denominators. A value that cannot be computed (zero denominator, rule not measurable) is
//! `None`, never zero. A window with too few active days or requests is not reliable; then no
//! change is stated at all, only the values. Model and front-end (`entrypoint`) mix of both
//! windows are reported as [`Limitation`]s. A change is shown, never explained: it is no
//! verdict on quality and no proof of a cause.

use crate::findings::{Evaluation, evaluate};
use crate::model::{Compaction, Dataset, Request};
use crate::report::summarize;
use crate::time::{DAY_MS, format_iso};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Active days a window needs before its values are compared. Uncalibrated assumption.
const MIN_ACTIVE_DAYS: u64 = 3;
/// Requests a window needs before its values are compared. Uncalibrated assumption.
const MIN_REQUESTS: u64 = 100;
/// Change of a model's or front end's share, in percentage points, that is named as a
/// limitation. Uncalibrated assumption.
const MIX_SHIFT_PCT: f64 = 10.0;

const METHOD: &str = "Both periods are read from the same logs with the same rules. Values are per \
active day or per request, not per calendar day. A change shows that a number moved, not why, and \
a higher or lower token use is not better or worse work.";

/// A period, half-open: `from_ms` is in, `to_ms` is out. Milliseconds since the epoch, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from_ms: i64,
    pub to_ms: i64,
}

/// A model's or front end's part of a window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    pub label: String,
    pub count: u64,
    pub share_pct: f64,
}

/// What one window holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowFacts {
    pub from: String,
    pub to: String,
    /// UTC calendar days the window touches; the first and last may be partial.
    pub days: u64,
    /// Days with at least one request.
    pub active_days: u64,
    pub days_without_logs: u64,
    pub requests: u64,
    pub input: u64,
    pub compactions: u64,
    /// Enough active days and requests to compare; see the limitations for what is missing.
    pub reliable: bool,
    /// Requests per model, most first.
    pub models: Vec<Share>,
    /// Sessions of the main conversation per front end (`entrypoint`), most first.
    pub entrypoints: Vec<Share>,
}

/// One figure in both windows. `change` and `change_pct` exist only when both values do and
/// both windows are reliable; `change_pct` also needs a non-zero earlier value. For metrics with
/// unit `%`, `change` is in percentage points.
#[derive(Debug, Clone, Serialize)]
pub struct Metric {
    pub name: &'static str,
    pub unit: &'static str,
    pub earlier: Option<f64>,
    pub later: Option<f64>,
    pub change: Option<f64>,
    pub change_pct: Option<f64>,
    /// What the value was divided by (or sampled from) in each window.
    pub denominator_earlier: u64,
    pub denominator_later: u64,
    pub denominator_unit: &'static str,
    /// Why a value is missing, where a rule could not measure it.
    pub note: Option<String>,
}

/// Something that limits what the comparison says. `kind`: `window-not-reliable`,
/// `empty-window`, `model-mix` or `entrypoint-mix`.
#[derive(Debug, Clone, Serialize)]
pub struct Limitation {
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    /// `compared`, `not_reliable` (a window is too small, values only) or `no_data` (a window
    /// holds no request).
    pub verdict: &'static str,
    pub method: &'static str,
    pub earlier: WindowFacts,
    pub later: WindowFacts,
    pub metrics: Vec<Metric>,
    pub limitations: Vec<Limitation>,
}

/// The windows of `--compare <days>`: the last `days` days up to `now_ms`, and the window of
/// the same length right before it.
pub fn windows_before(now_ms: i64, days: i64) -> (Window, Window) {
    let later = Window {
        from_ms: now_ms - days * DAY_MS,
        to_ms: now_ms,
    };
    let earlier = Window {
        from_ms: later.from_ms - days * DAY_MS,
        to_ms: later.from_ms,
    };
    (earlier, later)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn ratio(part: u64, whole: u64, scale: f64) -> Option<f64> {
    (whole > 0).then(|| part as f64 * scale / whole as f64)
}

/// A figure of one window with its denominator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Figure {
    pub value: Option<f64>,
    pub denominator: u64,
    pub note: Option<String>,
}

pub(crate) struct Side {
    pub(crate) facts: WindowFacts,
    pub(crate) figures: Vec<(&'static str, Figure)>,
}

fn shares(counts: BTreeMap<String, u64>) -> Vec<Share> {
    let total: u64 = counts.values().sum();
    let mut shares: Vec<Share> = counts
        .into_iter()
        .map(|(label, count)| Share {
            label,
            count,
            share_pct: round2(ratio(count, total, 100.0).unwrap_or(0.0)),
        })
        .collect();
    shares.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
    shares
}

/// The rule's figure from an evaluation, `None` with the reason when it was not measurable.
fn rule_figure(evaluation: &Evaluation, rule: &str, use_sample: bool) -> Figure {
    let status = evaluation.rules.iter().find(|r| r.rule == rule);
    match status.and_then(|s| s.measure.as_ref()) {
        Some(m) => Figure {
            value: Some(m.value),
            denominator: if use_sample { m.sample } else { m.denominator },
            note: None,
        },
        None => Figure {
            value: None,
            denominator: 0,
            note: status.and_then(|s| s.reason.clone()),
        },
    }
}

pub(crate) fn side(dataset: &Dataset, window: Window) -> Side {
    let in_window = |time_ms: i64| time_ms >= window.from_ms && time_ms < window.to_ms;
    let requests: Vec<&Request> = dataset
        .requests
        .iter()
        .filter(|r| in_window(r.time_ms))
        .collect();
    let compactions: Vec<&Compaction> = dataset
        .compactions
        .iter()
        .filter(|c| in_window(c.time_ms))
        .collect();
    let summary = summarize(&requests, &compactions);
    let active_days = summary.days.iter().filter(|d| d.requests > 0).count() as u64;
    let days = (((window.to_ms - 1).div_euclid(DAY_MS) - window.from_ms.div_euclid(DAY_MS)) + 1)
        .max(0) as u64;

    let mut models: BTreeMap<String, u64> = BTreeMap::new();
    for r in &requests {
        let model = if r.model.is_empty() {
            "(unknown)"
        } else {
            r.model.as_str()
        };
        *models.entry(model.to_owned()).or_default() += 1;
    }
    let fronts: HashMap<&str, Option<&str>> = dataset
        .session_starts
        .iter()
        .rev()
        .map(|s| (s.session.as_str(), s.entrypoint.as_deref()))
        .collect();
    let sessions: HashSet<&str> = requests
        .iter()
        .filter(|r| !r.sub)
        .map(|r| r.session.as_str())
        .collect();
    let mut entrypoints: BTreeMap<String, u64> = BTreeMap::new();
    for session in sessions {
        let front = fronts
            .get(session)
            .copied()
            .flatten()
            .unwrap_or("(not recorded)");
        *entrypoints.entry(front.to_owned()).or_default() += 1;
    }

    // The rules see the data up to the end of the window and report from its start: what
    // happened before still tells where a session started and what a pause was.
    let clipped = Dataset {
        requests: dataset
            .requests
            .iter()
            .filter(|r| r.time_ms < window.to_ms)
            .cloned()
            .collect(),
        compactions: dataset
            .compactions
            .iter()
            .filter(|c| c.time_ms < window.to_ms)
            .cloned()
            .collect(),
        ..dataset.clone()
    };
    let evaluation = evaluate(&clipped, Some(window.from_ms), None);

    let n = requests.len() as u64;
    let figure = |value: Option<f64>, denominator: u64| Figure {
        value,
        denominator,
        note: None,
    };
    let figures = vec![
        ("active_days", figure(Some(active_days as f64), days)),
        (
            "requests_per_active_day",
            figure(ratio(n, active_days, 1.0), active_days),
        ),
        ("input_per_request", figure(ratio(summary.input, n, 1.0), n)),
        (
            "cache_read_pct",
            figure(
                ratio(summary.cache_read, summary.input, 100.0),
                summary.input,
            ),
        ),
        (
            "compactions_per_1000_requests",
            figure(ratio(summary.compactions.total, n, 1000.0), n),
        ),
        (
            "median_start_context",
            rule_figure(&evaluation, "baseline-load", true),
        ),
        (
            "sub_agent_pct",
            figure(ratio(summary.sub_agent_requests, n, 100.0), n),
        ),
        (
            "cache_rebuilds",
            rule_figure(&evaluation, "cache-rebuild", false),
        ),
    ];
    Side {
        facts: WindowFacts {
            from: format_iso(window.from_ms),
            to: format_iso(window.to_ms),
            days,
            active_days,
            days_without_logs: days.saturating_sub(active_days),
            requests: n,
            input: summary.input,
            compactions: summary.compactions.total,
            reliable: active_days >= MIN_ACTIVE_DAYS && n >= MIN_REQUESTS,
            models: shares(models),
            entrypoints: shares(entrypoints),
        },
        figures,
    }
}

/// (name, unit, denominator unit) in output order.
pub(crate) const METRICS: [(&str, &str, &str); 8] = [
    ("active_days", "days", "calendar days in the window"),
    ("requests_per_active_day", "requests", "active days"),
    ("input_per_request", "tokens", "requests"),
    ("cache_read_pct", "%", "input tokens"),
    ("compactions_per_1000_requests", "compactions", "requests"),
    ("median_start_context", "tokens", "clean session starts"),
    ("sub_agent_pct", "%", "requests"),
    ("cache_rebuilds", "rebuilds", "pauses over 5 min"),
];

/// Limitation for a label whose share moved by [`MIX_SHIFT_PCT`] points or more.
fn mix_shift(
    kind: &'static str,
    what: &str,
    earlier: &[Share],
    later: &[Share],
) -> Option<Limitation> {
    let share = |list: &[Share], label: &str| {
        list.iter()
            .find(|s| s.label == label)
            .map_or(0.0, |s| s.share_pct)
    };
    let labels: Vec<&str> = earlier
        .iter()
        .chain(later)
        .map(|s| s.label.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let moved: Vec<String> = labels
        .into_iter()
        .filter_map(|label| {
            let (a, b) = (share(earlier, label), share(later, label));
            ((b - a).abs() >= MIX_SHIFT_PCT).then(|| format!("{label} {a:.0} % to {b:.0} %"))
        })
        .collect();
    (!moved.is_empty()).then(|| Limitation {
        kind,
        text: format!(
            "{what} differ between the windows ({}); a change in the numbers may come with it",
            moved.join(", ")
        ),
    })
}

fn describe(name: &str, facts: &WindowFacts) -> String {
    if facts.requests == 0 {
        return format!("the {name} window holds no requests");
    }
    let mut missing = Vec::new();
    if facts.active_days < MIN_ACTIVE_DAYS {
        missing.push(format!(
            "{} active days, {MIN_ACTIVE_DAYS} needed",
            facts.active_days
        ));
    }
    if facts.requests < MIN_REQUESTS {
        missing.push(format!(
            "{} requests, {MIN_REQUESTS} needed",
            facts.requests
        ));
    }
    format!(
        "the {name} window is too small to compare ({})",
        missing.join(", ")
    )
}

/// Compares two windows of one dataset. `earlier` and `later` are only names: nothing requires
/// `earlier` to precede `later`.
pub fn compare(dataset: &Dataset, earlier: Window, later: Window) -> Comparison {
    compare_sides(side(dataset, earlier), side(dataset, later))
}

/// The comparison of two measured windows, whether they were just read or restored from a capture.
pub(crate) fn compare_sides(a: Side, b: Side) -> Comparison {
    let both_reliable = a.facts.reliable && b.facts.reliable;
    let verdict = if a.facts.requests == 0 || b.facts.requests == 0 {
        "no_data"
    } else if both_reliable {
        "compared"
    } else {
        "not_reliable"
    };
    let metrics = METRICS
        .iter()
        .zip(a.figures.iter().zip(&b.figures))
        .map(|((name, unit, denominator_unit), ((_, fa), (_, fb)))| {
            let change = match (fa.value, fb.value) {
                (Some(x), Some(y)) if both_reliable => Some((x, y)),
                _ => None,
            };
            Metric {
                name,
                unit,
                earlier: fa.value.map(round2),
                later: fb.value.map(round2),
                change: change.map(|(x, y)| round2(y - x)),
                change_pct: change
                    .filter(|(x, _)| *x != 0.0)
                    .map(|(x, y)| round2((y - x) * 100.0 / x)),
                denominator_earlier: fa.denominator,
                denominator_later: fb.denominator,
                denominator_unit,
                note: fa.note.clone().or_else(|| fb.note.clone()),
            }
        })
        .collect();

    let mut limitations = Vec::new();
    for (name, facts) in [("earlier", &a.facts), ("later", &b.facts)] {
        if facts.requests == 0 {
            limitations.push(Limitation {
                kind: "empty-window",
                text: describe(name, facts),
            });
        } else if !facts.reliable {
            limitations.push(Limitation {
                kind: "window-not-reliable",
                text: describe(name, facts),
            });
        }
    }
    if a.facts.requests > 0 && b.facts.requests > 0 {
        limitations.extend(mix_shift(
            "model-mix",
            "the models used",
            &a.facts.models,
            &b.facts.models,
        ));
        limitations.extend(mix_shift(
            "entrypoint-mix",
            "the front ends (entrypoint) of the sessions",
            &a.facts.entrypoints,
            &b.facts.entrypoints,
        ));
    }
    Comparison {
        verdict,
        method: METHOD,
        earlier: a.facts,
        later: b.facts,
        metrics,
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionStart;

    fn req(session: &str, day: i64, hour: i64, model: &str, fresh: u64, read: u64) -> Request {
        Request {
            session: session.into(),
            project: "demo".into(),
            source: 0,
            message_id: String::new(),
            time_ms: day * DAY_MS + hour * 3_600_000,
            model: model.into(),
            sub: false,
            fresh,
            cache_read: read,
            cache_write_5m: 0,
            cache_write_1h: 0,
            output: 1,
            window: None,
        }
    }

    fn win(from_day: i64, to_day: i64) -> Window {
        Window {
            from_ms: from_day * DAY_MS,
            to_ms: to_day * DAY_MS,
        }
    }

    /// `per_day` requests on each of `days` days from `first`, 100 tokens each, `read` cached.
    fn busy(first: i64, days: i64, per_day: i64, model: &str, read: u64) -> Vec<Request> {
        (first..first + days)
            .flat_map(|d| {
                (0..per_day).map(move |i| req(&format!("s{d}"), d, i % 20, model, 100 - read, read))
            })
            .collect()
    }

    fn dataset(requests: Vec<Request>) -> Dataset {
        Dataset {
            requests,
            ..Dataset::default()
        }
    }

    fn metric<'a>(c: &'a Comparison, name: &str) -> &'a Metric {
        c.metrics.iter().find(|m| m.name == name).unwrap()
    }

    #[test]
    fn known_windows_give_known_changes_and_denominators() {
        // Earlier: 4 days x 50 requests, 50 % cached. Later: 4 days x 100, 80 % cached.
        let mut requests = busy(0, 4, 50, "m", 50);
        requests.extend(busy(10, 4, 100, "m", 80));
        let c = compare(&dataset(requests), win(0, 8), win(8, 14));
        assert_eq!(c.verdict, "compared");
        assert_eq!((c.earlier.requests, c.later.requests), (200, 400));
        assert_eq!((c.earlier.active_days, c.later.active_days), (4, 4));
        assert_eq!((c.earlier.days, c.later.days), (8, 6));
        assert_eq!(c.earlier.days_without_logs, 4);
        let m = metric(&c, "requests_per_active_day");
        assert_eq!((m.earlier, m.later), (Some(50.0), Some(100.0)));
        assert_eq!((m.change, m.change_pct), (Some(50.0), Some(100.0)));
        assert_eq!((m.denominator_earlier, m.denominator_later), (4, 4));
        let m = metric(&c, "cache_read_pct");
        assert_eq!(
            (m.earlier, m.later, m.change),
            (Some(50.0), Some(80.0), Some(30.0))
        );
        assert_eq!(m.change_pct, Some(60.0));
        assert_eq!(m.denominator_earlier, 200 * 100);
        assert_eq!(metric(&c, "input_per_request").change, Some(0.0));
        assert!(c.limitations.is_empty(), "{:?}", c.limitations);
    }

    #[test]
    fn empty_and_small_windows_state_no_change() {
        let c = compare(&dataset(busy(8, 4, 50, "m", 50)), win(0, 8), win(8, 14));
        assert_eq!(c.verdict, "no_data");
        assert!(c.metrics.iter().all(|m| m.change.is_none()));
        assert_eq!(metric(&c, "requests_per_active_day").earlier, None);
        assert_eq!(c.limitations[0].kind, "empty-window");

        // Two active days and 40 requests: values stay, the change does not.
        let mut requests = busy(0, 2, 20, "m", 50);
        requests.extend(busy(8, 4, 50, "m", 50));
        let c = compare(&dataset(requests), win(0, 8), win(8, 14));
        assert_eq!(c.verdict, "not_reliable");
        let m = metric(&c, "requests_per_active_day");
        assert_eq!((m.earlier, m.later), (Some(20.0), Some(50.0)));
        assert!(m.change.is_none() && m.change_pct.is_none());
        assert_eq!(c.limitations[0].kind, "window-not-reliable");
    }

    #[test]
    fn zero_earlier_value_has_no_relative_change() {
        let mut requests = busy(0, 4, 50, "m", 0);
        requests.extend(busy(8, 4, 50, "m", 50));
        let c = compare(&dataset(requests), win(0, 8), win(8, 14));
        let m = metric(&c, "cache_read_pct");
        assert_eq!((m.earlier, m.change), (Some(0.0), Some(50.0)));
        assert_eq!(m.change_pct, None);
    }

    #[test]
    fn unmeasurable_rules_stay_unknown_not_zero() {
        let mut requests = busy(0, 4, 50, "m", 50);
        requests.extend(busy(8, 4, 50, "m", 50));
        let c = compare(&dataset(requests), win(0, 8), win(8, 14));
        for name in ["median_start_context", "cache_rebuilds"] {
            let m = metric(&c, name);
            assert!(m.earlier.is_none() && m.later.is_none() && m.change.is_none());
            assert!(m.note.is_some(), "{name}");
        }
    }

    #[test]
    fn model_and_front_end_changes_are_limitations() {
        let mut requests = busy(0, 4, 50, "old", 50);
        requests.extend(busy(8, 4, 50, "new", 50));
        let mut data = dataset(requests);
        for d in (0..4).chain(8..12) {
            data.session_starts.push(SessionStart {
                session: format!("s{d}"),
                source: 0,
                clean: true,
                reason: None,
                entrypoint: Some(if d < 4 { "cli" } else { "desktop" }.into()),
                cwd: None,
                parent: None,
            });
        }
        let c = compare(&data, win(0, 8), win(8, 14));
        let kinds: Vec<&str> = c.limitations.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, ["model-mix", "entrypoint-mix"]);
        assert!(c.limitations[0].text.contains("old 100 % to 0 %"));
        assert_eq!(c.verdict, "compared");
    }

    #[test]
    fn rules_see_only_their_own_window() {
        // Ten clean session starts in the later window only, 40k each: a median there, none before.
        let mut data = dataset(busy(0, 4, 50, "m", 50));
        for i in 0..10 {
            data.requests
                .push(req(&format!("n{i}"), 9, i, "m", 40_000, 0));
            data.session_starts.push(SessionStart {
                session: format!("n{i}"),
                source: 0,
                clean: true,
                reason: None,
                entrypoint: None,
                cwd: None,
                parent: None,
            });
        }
        data.requests.extend(busy(10, 3, 50, "m", 50));
        let c = compare(&data, win(0, 8), win(8, 14));
        let m = metric(&c, "median_start_context");
        assert_eq!(m.earlier, None);
        assert_eq!(m.later, Some(40_000.0));
        assert_eq!(m.denominator_later, 10);
    }
}
