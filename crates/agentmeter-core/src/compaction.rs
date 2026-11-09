//! `--compaction-causes`: what the logs show about when and where compactions happen, and, if
//! asked for, how today's auto-compact setting compares with the sizes that were observed.
//!
//! Everything is counted from the compaction events the summary already counts (and the timeline
//! of `--transitions` builds on); nothing is recounted from another source. A correlation is
//! worded as a hint with the lever it points to, never as the cause. Missing trigger and size
//! stay unknown. The setting is today's file content; it is never attributed to the past.

use crate::findings::{BIG_CONTEXT_TOKENS, LONG_SESSION_MS, evaluate, quantile, round1};
use crate::format::{count, tokens};
use crate::model::{Compaction, Dataset, Request};
use crate::time::HOUR_MS;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Automatic compactions of one model with a known size from which a narrow band is reported.
/// Uncalibrated assumption.
const BAND_MIN_SAMPLE: usize = 3;
/// Width of the middle half (third minus first quartile) up to this share of the median counts
/// as narrow. Uncalibrated assumption.
const BAND_MAX_SPREAD_PCT: u64 = 10;
/// Share of all compactions from which one project or the long sessions are reported, and the
/// least number of compactions for it. Uncalibrated assumptions.
const CONCENTRATION_SHARE_PCT: u64 = 50;
const CONCENTRATION_MIN: u64 = 3;
/// Median size before compaction as a share of the setting: within this range the setting and
/// the observed sizes fit; below it compactions began well under the limit, above it they were
/// larger than it. Uncalibrated assumptions.
const FIT_LOW_PCT: f64 = 70.0;
const FIT_HIGH_PCT: f64 = 105.0;
/// Range Claude Code accepts for `autoCompactWindow`.
const WINDOW_MIN: u64 = 100_000;
const WINDOW_MAX: u64 = 1_000_000;

/// The documentation the setting keys and their meaning come from.
pub const SETTINGS_REFERENCE: &str = "Claude Code settings reference and model configuration \
    docs (code.claude.com), as of 2026-11: `autoCompactWindow` (100000 to 1000000 tokens, any \
    settings file), `modelSettings.<model>.autoCompactWindow` (a number or \"auto\"; needs Claude \
    Code 2.1.288 or later; `/autocompact` saves it there, before 2.1.288 the top-level key), \
    `autoCompactEnabled`";

const METHOD: &str = "compactions as the summary counts them, grouped by project, model and \
                      session; rates are per 1,000 main-conversation requests; a correlation is a \
                      hint with a lever to check, never the cause";

#[derive(Debug, Clone, Serialize)]
pub struct Spread {
    pub min: u64,
    pub q1: u64,
    pub median: u64,
    pub q3: u64,
    pub max: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Triggers {
    pub auto: u64,
    pub manual: u64,
    /// The log does not say (always for Codex).
    pub unknown: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    /// Project name, model name or session id; `all` for the totals.
    pub key: String,
    pub compactions: u64,
    /// Main-conversation requests of the group in the period.
    pub requests: u64,
    /// Compactions per 1,000 of those requests; `null` without requests.
    pub per_1000_requests: Option<f64>,
    pub triggers: Triggers,
    /// Size before compaction; `null` without compactions.
    pub tokens_before: Option<Spread>,
    /// Compactions whose size after compaction the log does not state.
    pub without_tokens_after: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionGroup {
    #[serde(flatten)]
    pub group: Group,
    pub project: String,
    /// First to last main-conversation request in the period, idle time included.
    pub span_ms: i64,
    pub peak_context: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    /// `size_band`, `long_sessions` or `concentrated`.
    pub kind: &'static str,
    /// Model name, project name or `all`.
    pub scope: String,
    pub message: String,
    pub compactions: u64,
    /// Compactions of the scope the measure is taken from.
    pub of: u64,
    /// Which lever the observation points to: `setting_or_window` or `working_style`.
    pub lever: &'static str,
    /// Always `hint`: a pattern in the logs, not a cause.
    pub certainty: &'static str,
    /// The rule that already reports the same behaviour; this is a count of compactions in it,
    /// not a second finding.
    pub see_rule: Option<&'static str>,
    pub check: String,
}

/// A rule of the findings as it reports, shown next to the compactions without recounting.
#[derive(Debug, Clone, Serialize)]
pub struct RelatedRule {
    pub rule: &'static str,
    pub status: &'static str,
    pub message: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsFile {
    pub path: String,
    /// `user`, `project` or `local`.
    pub scope: &'static str,
    pub project: Option<String>,
    /// `read`, `missing`, `unreadable` or `invalid_json`.
    pub state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowSetting {
    pub file: String,
    pub scope: &'static str,
    pub project: Option<String>,
    /// The model the entry is for (`modelSettings`); `null` for every model.
    pub model: Option<String>,
    /// Tokens as written; `null` for `"auto"`.
    pub tokens: Option<u64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwitchSetting {
    pub file: String,
    pub scope: &'static str,
    pub project: Option<String>,
    /// `autoCompactEnabled` as written.
    pub enabled: bool,
}

/// What the Claude Code settings files hold today. Read only on request.
#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    /// The documentation the keys and their version bound come from.
    pub reference: &'static str,
    /// Always says that this is the current content of the files, not the history.
    pub as_of: &'static str,
    pub files: Vec<SettingsFile>,
    pub windows: Vec<WindowSetting>,
    pub auto_compact_enabled: Vec<SwitchSetting>,
    /// Projects whose working directory is unknown or gone: their project and local settings
    /// could not be looked at, so what they hold is unknown, not absent.
    pub projects_not_read: Vec<String>,
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            reference: SETTINGS_REFERENCE,
            as_of: "the files as they are now; not the configuration of any past session",
            files: Vec::new(),
            windows: Vec::new(),
            auto_compact_enabled: Vec::new(),
            projects_not_read: Vec::new(),
        }
    }

    /// The window that applies today to `model` in `project` by the files read: local before
    /// project before user settings; in a file the entry for the model before the general one.
    fn window_for(&self, project: &str, model: &str) -> Option<&WindowSetting> {
        ["local", "project", "user"].iter().find_map(|scope| {
            let of_scope = || {
                self.windows.iter().filter(move |w| {
                    w.scope == *scope && w.project.as_deref().is_none_or(|p| p == project)
                })
            };
            of_scope()
                .find(|w| w.model.as_deref().is_some_and(|m| model_matches(m, model)))
                .or_else(|| of_scope().find(|w| w.model.is_none()))
        })
    }

    fn enabled_for(&self, project: &str) -> Option<bool> {
        ["local", "project", "user"].iter().find_map(|scope| {
            self.auto_compact_enabled
                .iter()
                .find(|s| s.scope == *scope && s.project.as_deref().is_none_or(|p| p == project))
                .map(|s| s.enabled)
        })
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings::new()
    }
}

/// Claude Code files an entry under the canonical model name and matches the alias, the dated and
/// the `[1m]` ids to it; the logged id matches when it starts with that name.
fn model_matches(setting_model: &str, logged: &str) -> bool {
    let strip = |m: &str| m.to_lowercase().replace("[1m]", "");
    let key = strip(setting_model);
    !key.is_empty() && strip(logged).starts_with(&key)
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub project: String,
    pub model: String,
    /// The window that applies today by the files read; `null` when none is set.
    pub setting: Option<WindowSetting>,
    /// `autoCompactEnabled` that applies today, `null` when not set.
    pub auto_compact_enabled: Option<bool>,
    /// Automatic compactions of the pair with a known size before compaction.
    pub automatic_compactions: u64,
    pub median_tokens_before: Option<u64>,
    pub max_tokens_before: Option<u64>,
    /// Median size before compaction as a percent of the setting.
    pub median_share_of_window_pct: Option<f64>,
    /// Automatic compactions larger than the setting.
    pub above_window: u64,
    /// `within`, `below`, `above`, `no_setting`, `auto_value` or `not_measurable`.
    pub fit: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CausesReport {
    pub method: &'static str,
    pub totals: Group,
    pub projects: Vec<Group>,
    pub models: Vec<Group>,
    /// Sessions with at least one compaction.
    pub sessions: Vec<SessionGroup>,
    pub observations: Vec<Observation>,
    pub related_rules: Vec<RelatedRule>,
    /// Only with `--compaction-settings` for an agent whose settings are read.
    pub settings: Option<Settings>,
    pub settings_comparison: Vec<Comparison>,
    pub notes: Vec<String>,
}

struct Item<'a> {
    c: &'a Compaction,
    model: String,
}

fn model_of(stream: &[&Request], at_ms: i64) -> String {
    let i = stream.partition_point(|r| r.time_ms <= at_ms);
    let r = i
        .checked_sub(1)
        .and_then(|before| stream.get(before))
        .or_else(|| stream.get(i));
    r.map(|r| r.model.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn spread(mut v: Vec<u64>) -> Option<Spread> {
    v.sort_unstable();
    (!v.is_empty()).then(|| Spread {
        min: v[0],
        q1: quantile(&v, 0.25),
        median: quantile(&v, 0.5),
        q3: quantile(&v, 0.75),
        max: v[v.len() - 1],
    })
}

fn group(key: &str, items: &[&Item], requests: u64) -> Group {
    let mut triggers = Triggers::default();
    for i in items {
        match i.c.auto {
            Some(true) => triggers.auto += 1,
            Some(false) => triggers.manual += 1,
            None => triggers.unknown += 1,
        }
    }
    let n = items.len() as u64;
    Group {
        key: key.to_owned(),
        compactions: n,
        requests,
        per_1000_requests: (requests > 0).then(|| round1(n as f64 * 1000.0 / requests as f64)),
        triggers,
        tokens_before: spread(items.iter().map(|i| i.c.tokens_before).collect()),
        without_tokens_after: items.iter().filter(|i| i.c.tokens_after.is_none()).count() as u64,
    }
}

fn percent(part: u64, whole: u64) -> f64 {
    round1(part as f64 * 100.0 / whole.max(1) as f64)
}

/// Builds the report for the compactions at or after `since_ms`. `settings` are today's settings
/// files when they were read; `settings_requested` tells that they were asked for, so that an
/// agent whose settings are not read says so.
pub fn analyse(
    dataset: &Dataset,
    since_ms: Option<i64>,
    settings: Option<Settings>,
    settings_requested: bool,
) -> CausesReport {
    let in_period = |t: i64| since_ms.is_none_or(|s| t >= s);
    let mut streams: BTreeMap<usize, Vec<&Request>> = BTreeMap::new();
    for r in dataset.requests.iter().filter(|r| !r.sub) {
        streams.entry(r.source).or_default().push(r);
    }
    for v in streams.values_mut() {
        v.sort_by_key(|r| r.time_ms);
    }
    let main: Vec<&Request> = dataset
        .requests
        .iter()
        .filter(|r| !r.sub && in_period(r.time_ms))
        .collect();
    let model_name = |r: &Request| {
        if r.model.is_empty() {
            "unknown".to_owned()
        } else {
            r.model.clone()
        }
    };
    let mut requests_by_project: BTreeMap<&str, u64> = BTreeMap::new();
    let mut requests_by_model: BTreeMap<String, u64> = BTreeMap::new();
    // session -> (first, last, requests, peak context)
    let mut spans: BTreeMap<&str, (i64, i64, u64, u64)> = BTreeMap::new();
    for r in &main {
        *requests_by_project.entry(r.project.as_str()).or_default() += 1;
        *requests_by_model.entry(model_name(r)).or_default() += 1;
        let s = spans
            .entry(r.session.as_str())
            .or_insert((r.time_ms, r.time_ms, 0, 0));
        s.0 = s.0.min(r.time_ms);
        s.1 = s.1.max(r.time_ms);
        s.2 += 1;
        s.3 = s.3.max(r.context());
    }

    let items: Vec<Item> = dataset
        .compactions
        .iter()
        .filter(|c| in_period(c.time_ms))
        .map(|c| Item {
            c,
            model: model_of(
                streams.get(&c.source).map_or(&[][..], Vec::as_slice),
                c.time_ms,
            ),
        })
        .collect();
    let all: Vec<&Item> = items.iter().collect();
    let totals = group("all", &all, main.len() as u64);

    let rows = |by: &dyn Fn(&Item) -> String, requests: &dyn Fn(&str) -> u64| {
        let mut map: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
        for i in &items {
            map.entry(by(i)).or_default().push(i);
        }
        let mut rows: Vec<Group> = map.iter().map(|(k, v)| group(k, v, requests(k))).collect();
        rows.sort_by(|a, b| (b.compactions, &a.key).cmp(&(a.compactions, &b.key)));
        rows
    };
    let projects = rows(&|i| i.c.project.clone(), &|k| {
        requests_by_project.get(k).copied().unwrap_or(0)
    });
    let models = rows(&|i| i.model.clone(), &|k| {
        requests_by_model.get(k).copied().unwrap_or(0)
    });
    let mut sessions: Vec<SessionGroup> = rows(&|i| i.c.session.clone(), &|k| {
        spans.get(k).map_or(0, |s| s.2)
    })
    .into_iter()
    .map(|g| {
        let project = items
            .iter()
            .find(|i| i.c.session == g.key)
            .map(|i| i.c.project.clone())
            .unwrap_or_default();
        let (first, last, _, peak) = spans.get(g.key.as_str()).copied().unwrap_or((0, 0, 0, 0));
        SessionGroup {
            group: g,
            project,
            span_ms: last - first,
            peak_context: peak,
        }
    })
    .collect();
    sessions.sort_by(|a, b| {
        (b.group.compactions, &a.group.key).cmp(&(a.group.compactions, &b.group.key))
    });

    let mut observations = Vec::new();
    let total = items.len() as u64;
    size_bands(&items, &mut observations);
    long_sessions(&sessions, total, &mut observations);
    concentrated(
        &projects,
        total,
        requests_by_project.len(),
        &mut observations,
    );

    let evaluation = evaluate(dataset, since_ms, None);
    let related_rules = evaluation
        .rules
        .iter()
        .filter(|r| ["compact-thrash", "big-context", "long-session"].contains(&r.rule))
        .map(|r| RelatedRule {
            rule: r.rule,
            status: r.status,
            message: r.message.clone(),
            reason: r.reason.clone(),
        })
        .collect();

    let settings_comparison = settings
        .as_ref()
        .map(|s| comparisons(s, &items))
        .unwrap_or_default();
    let mut notes = vec![
        "Trigger and size are taken from the compaction event. Where the log does not state them \
         (Codex: always the trigger and the size after compaction) they stay unknown and are \
         counted as such; for Codex the size before is the context of the last request \
         before the event."
            .to_owned(),
        "A compaction event names no model; each compaction takes the model of the request \
         just before it."
            .to_owned(),
        "Counts are the compactions the summary counts; `compact-thrash` keeps judging how close \
         together they are, `long-session` and `big-context` how long and how large sessions are. \
         The observations only count compactions in those places."
            .to_owned(),
    ];
    if settings.is_some() {
        notes.push(
            "Settings are the files as they are now. They are not attributed to compactions of \
             the past: the setting may be newer than the sizes it is compared with. Managed \
             settings, CLAUDE_CODE_AUTO_COMPACT_WINDOW and --autocompact are not visible here \
             and take precedence over the files."
                .to_owned(),
        );
        if settings.as_ref().is_some_and(|s| {
            s.files
                .iter()
                .any(|f| f.state == "unreadable" || f.state == "invalid_json")
        }) {
            notes.push(
                "At least one settings file could not be read or parsed; what it holds is \
                 unknown, not absent."
                    .to_owned(),
            );
        }
    } else if settings_requested {
        notes.push("Settings are not read for this agent.".to_owned());
    } else {
        notes.push(
            "No settings file was read: add --compaction-settings to compare today's auto-compact \
             setting with the sizes above."
                .to_owned(),
        );
    }
    CausesReport {
        method: METHOD,
        totals,
        projects,
        models,
        sessions,
        observations,
        related_rules,
        settings,
        settings_comparison,
        notes,
    }
}

/// Automatic compactions of a model that began within a narrow band of sizes.
fn size_bands(items: &[Item], out: &mut Vec<Observation>) {
    let mut by_model: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for i in items.iter().filter(|i| i.c.auto == Some(true)) {
        by_model
            .entry(i.model.as_str())
            .or_default()
            .push(i.c.tokens_before);
    }
    for (model, sizes) in by_model {
        let n = sizes.len();
        let Some(s) = spread(sizes).filter(|_| n >= BAND_MIN_SAMPLE) else {
            continue;
        };
        if s.median == 0 || (s.q3 - s.q1) * 100 > BAND_MAX_SPREAD_PCT * s.median {
            continue;
        }
        out.push(Observation {
            kind: "size_band",
            scope: model.to_owned(),
            message: format!(
                "{} automatic compactions of {model} began at {} to {} (median {}): a narrow band, \
                 as a fixed limit produces; the logs do not show whether the model's window or a \
                 setting set it",
                count(n as u64),
                tokens(s.q1),
                tokens(s.q3),
                tokens(s.median)
            ),
            compactions: n as u64,
            of: n as u64,
            lever: "setting_or_window",
            certainty: "hint",
            see_rule: (s.median >= BIG_CONTEXT_TOKENS).then_some("big-context"),
            check: "compare the band with `autoCompactWindow` and the model's window (add \
                    --compaction-settings to read today's files); after a change the effect \
                    shows as compactions per 1,000 requests over a later period (--since)"
                .into(),
        });
    }
}

/// Compactions in sessions that span more than a day: the same sessions `long-session` lists.
fn long_sessions(sessions: &[SessionGroup], total: u64, out: &mut Vec<Observation>) {
    let long: Vec<&SessionGroup> = sessions
        .iter()
        .filter(|s| s.span_ms > LONG_SESSION_MS)
        .collect();
    let n: u64 = long.iter().map(|s| s.group.compactions).sum();
    if n < CONCENTRATION_MIN || n * 100 < CONCENTRATION_SHARE_PCT * total {
        return;
    }
    let longest = long.iter().max_by_key(|s| s.span_ms).unwrap();
    out.push(Observation {
        kind: "long_sessions",
        scope: "all".to_owned(),
        message: format!(
            "{} of {} compactions ({:.0}%) fell in {} sessions that span more than 24 h (longest \
             {:.0} h, {} requests); idle time is included in the span",
            count(n),
            count(total),
            percent(n, total),
            count(long.len() as u64),
            longest.span_ms as f64 / HOUR_MS as f64,
            count(longest.group.requests)
        ),
        compactions: n,
        of: total,
        lever: "working_style",
        certainty: "hint",
        see_rule: Some("long-session"),
        check: "a new session at a change of topic starts with a smaller context; check the \
                longest sessions for topics that changed, and compare compactions per 1,000 \
                requests over a later period (--since)"
            .into(),
    });
}

/// One project holding half of the compactions or more, with others present.
fn concentrated(
    projects: &[Group],
    total: u64,
    projects_with_requests: usize,
    out: &mut Vec<Observation>,
) {
    if projects_with_requests < 2 {
        return;
    }
    for p in projects {
        if p.compactions < CONCENTRATION_MIN
            || p.compactions * 100 < CONCENTRATION_SHARE_PCT * total
        {
            continue;
        }
        out.push(Observation {
            kind: "concentrated",
            scope: p.key.clone(),
            message: format!(
                "{} of {} compactions ({:.0}%) are in project {}{}",
                count(p.compactions),
                count(total),
                percent(p.compactions, total),
                p.key,
                p.per_1000_requests
                    .map_or(String::new(), |r| format!(", {r} per 1,000 requests"))
            ),
            compactions: p.compactions,
            of: total,
            lever: "working_style",
            certainty: "hint",
            see_rule: None,
            check: "look at the sessions of this project for very long tasks or large tool \
                    outputs (--tool-calls reports large-output); the other projects set no limit \
                    different from this one's, so the way of working there is the lever"
                .into(),
        });
    }
}

/// Today's window against the sizes observed, per project and model with compactions.
fn comparisons(settings: &Settings, items: &[Item]) -> Vec<Comparison> {
    let pairs: BTreeSet<(&str, &str)> = items
        .iter()
        .map(|i| (i.c.project.as_str(), i.model.as_str()))
        .collect();
    let mut out = Vec::new();
    for (project, model) in pairs {
        let sizes: Vec<u64> = items
            .iter()
            .filter(|i| i.c.project == project && i.model == model && i.c.auto == Some(true))
            .map(|i| i.c.tokens_before)
            .collect();
        let setting = settings.window_for(project, model);
        let mut row = Comparison {
            project: project.to_owned(),
            model: model.to_owned(),
            setting: setting.cloned(),
            auto_compact_enabled: settings.enabled_for(project),
            automatic_compactions: sizes.len() as u64,
            median_tokens_before: None,
            max_tokens_before: None,
            median_share_of_window_pct: None,
            above_window: 0,
            fit: "not_measurable",
            message: String::new(),
        };
        let observed = spread(sizes.clone());
        row.median_tokens_before = observed.as_ref().map(|s| s.median);
        row.max_tokens_before = observed.as_ref().map(|s| s.max);
        let Some(setting) = setting else {
            row.fit = "no_setting";
            row.message = "no `autoCompactWindow` in the files read: Claude Code picks the window \
                           for the model, so there is nothing to compare"
                .into();
            out.push(row);
            continue;
        };
        let Some(window) = setting.tokens else {
            row.fit = "auto_value";
            row.message = "the setting is \"auto\": the window tuned for the model, a number the \
                           logs do not state"
                .into();
            out.push(row);
            continue;
        };
        let Some(observed) = observed else {
            row.message = "no automatic compaction with a known size to compare".into();
            out.push(row);
            continue;
        };
        row.above_window = sizes.iter().filter(|s| **s > window).count() as u64;
        let share = observed.median as f64 * 100.0 / window as f64;
        row.median_share_of_window_pct = Some(round1(share));
        let (fit, what) = if share < FIT_LOW_PCT {
            (
                "below",
                "compactions began well under it: another limit, a smaller window then, or \
                 another trigger",
            )
        } else if share > FIT_HIGH_PCT {
            (
                "above",
                "compactions were larger than it: the setting probably was not in force then",
            )
        } else {
            ("within", "the observed sizes are in line with it")
        };
        row.fit = fit;
        row.message = format!(
            "today's window {} against a median {} before {} automatic compactions ({:.0}% of the \
             window, {} above it): {what}; a hint, not proof, and not claimed for the past",
            tokens(window),
            tokens(observed.median),
            count(sizes.len() as u64),
            share,
            count(row.above_window)
        );
        out.push(row);
    }
    out
}

/// Reads the value of an `autoCompactWindow` key: a number of tokens or `"auto"`.
/// `None` is an unusable value; tokens `None` inside is `"auto"`.
pub fn window_value(value: &serde_json::Value) -> Option<(Option<u64>, Option<String>)> {
    if value.as_str() == Some("auto") {
        return Some((None, None));
    }
    let n = value.as_u64()?;
    let note = (!(WINDOW_MIN..=WINDOW_MAX).contains(&n)).then(|| {
        format!(
            "outside {} to {}: Claude Code limits it to that range",
            count(WINDOW_MIN),
            count(WINDOW_MAX)
        )
    });
    Some((Some(n), note))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(project: &str, session: &str, model: &str, t: i64) -> Request {
        Request {
            session: session.into(),
            project: project.into(),
            source: 0,
            message_id: format!("m{t}"),
            time_ms: t,
            model: model.into(),
            sub: false,
            fresh: 1000,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            output: 1,
            window: None,
        }
    }

    fn comp(project: &str, session: &str, t: i64, before: u64, auto: Option<bool>) -> Compaction {
        Compaction {
            session: session.into(),
            project: project.into(),
            source: 0,
            event_id: format!("c{t}"),
            time_ms: t,
            tokens_before: before,
            tokens_after: None,
            auto,
        }
    }

    fn dataset(requests: Vec<Request>, compactions: Vec<Compaction>) -> Dataset {
        Dataset {
            requests,
            compactions,
            files: vec!["f.jsonl".into()],
            ..Dataset::default()
        }
    }

    #[test]
    fn groups_by_project_model_and_session_with_rates_and_unknowns() {
        let mut requests = Vec::new();
        for t in 0..10 {
            requests.push(req("a", "s1", "m-big", t * 1000));
        }
        for t in 0..5 {
            requests.push(req("b", "s2", "m-small", 100_000 + t * 1000));
        }
        let compactions = vec![
            comp("a", "s1", 3500, 190_000, Some(true)),
            comp("a", "s1", 6500, 200_000, Some(true)),
            comp("a", "s1", 8500, 100_000, Some(false)),
            comp("b", "s2", 102_500, 50_000, None),
        ];
        let r = analyse(&dataset(requests, compactions), None, None, false);
        assert_eq!(r.totals.compactions, 4);
        let a = r.projects.iter().find(|p| p.key == "a").unwrap();
        assert_eq!((a.compactions, a.requests), (3, 10));
        assert_eq!(a.per_1000_requests, Some(300.0));
        assert_eq!(
            (a.triggers.auto, a.triggers.manual, a.triggers.unknown),
            (2, 1, 0)
        );
        let s = a.tokens_before.as_ref().unwrap();
        assert_eq!((s.min, s.median, s.max), (100_000, 190_000, 200_000));
        let b = r.projects.iter().find(|p| p.key == "b").unwrap();
        assert_eq!(b.triggers.unknown, 1);
        assert_eq!(b.without_tokens_after, 1);
        let model = |k: &str| r.models.iter().find(|m| m.key == k).unwrap();
        assert_eq!(model("m-big").compactions, 3);
        assert_eq!(model("m-small").per_1000_requests, Some(200.0));
        assert_eq!(r.sessions.len(), 2);
        assert_eq!(r.sessions[0].group.key, "s1");
        assert_eq!(r.sessions[0].span_ms, 9000);
        assert!(r.notes.iter().any(|n| n.contains("--compaction-settings")));
    }

    #[test]
    fn narrow_band_of_automatic_compactions_is_a_hint_for_the_limit() {
        let requests = (0..10).map(|t| req("a", "s", "m", t * 1000)).collect();
        let compactions = [195_000, 198_000, 200_000, 199_000]
            .iter()
            .enumerate()
            .map(|(i, b)| comp("a", "s", 500 + i as i64 * 2000, *b, Some(true)))
            .collect();
        let r = analyse(&dataset(requests, compactions), None, None, false);
        let o = r
            .observations
            .iter()
            .find(|o| o.kind == "size_band")
            .unwrap();
        assert_eq!(o.lever, "setting_or_window");
        assert_eq!(o.certainty, "hint");
        assert_eq!(o.compactions, 4);
        // Wide spread: no band; unknown trigger: not counted.
        let requests = (0..10).map(|t| req("a", "s", "m", t * 1000)).collect();
        let compactions = [100_000, 150_000, 200_000]
            .iter()
            .enumerate()
            .map(|(i, b)| comp("a", "s", 500 + i as i64 * 2000, *b, Some(true)))
            .chain((0..5).map(|i| comp("a", "s", 9000 + i, 200_000, None)))
            .collect();
        let r = analyse(&dataset(requests, compactions), None, None, false);
        assert!(r.observations.iter().all(|o| o.kind != "size_band"));
    }

    #[test]
    fn long_sessions_and_one_project_are_working_style_hints_pointing_to_the_rule() {
        let day = 24 * HOUR_MS;
        let mut requests = vec![
            req("a", "long", "m", 0),
            req("a", "long", "m", 2 * day),
            req("b", "short", "m", 0),
            req("b", "short", "m", 1000),
        ];
        requests.push(req("a", "long", "m", day));
        let compactions = vec![
            comp("a", "long", 100, 150_000, Some(true)),
            comp("a", "long", day, 160_000, Some(true)),
            comp("a", "long", 2 * day - 1, 170_000, Some(true)),
        ];
        let r = analyse(&dataset(requests, compactions), None, None, false);
        let long = r
            .observations
            .iter()
            .find(|o| o.kind == "long_sessions")
            .unwrap();
        assert_eq!(long.lever, "working_style");
        assert_eq!(long.see_rule, Some("long-session"));
        let concentrated = r
            .observations
            .iter()
            .find(|o| o.kind == "concentrated")
            .unwrap();
        assert_eq!(concentrated.scope, "a");
        // The existing rules are shown as they report, not recounted.
        assert!(r.related_rules.iter().any(|x| x.rule == "long-session"));
        // A single project is never "concentrated".
        let one = analyse(
            &dataset(
                vec![req("a", "s", "m", 0)],
                (0..4).map(|i| comp("a", "s", i, 1, Some(true))).collect(),
            ),
            None,
            None,
            false,
        );
        assert!(one.observations.iter().all(|o| o.kind != "concentrated"));
    }

    fn window(
        scope: &'static str,
        project: Option<&str>,
        model: Option<&str>,
        tokens: Option<u64>,
    ) -> WindowSetting {
        WindowSetting {
            file: format!("{scope}.json"),
            scope,
            project: project.map(str::to_owned),
            model: model.map(str::to_owned),
            tokens,
            note: None,
        }
    }

    #[test]
    fn setting_applies_by_scope_and_model_and_is_a_hint_against_the_sizes() {
        let requests = (0..10)
            .map(|t| req("a", "s", "claude-opus-4-5-20251101", t * 1000))
            .collect();
        let compactions = (0..3)
            .map(|i| comp("a", "s", 500 + i * 2000, 190_000, Some(true)))
            .collect();
        let mut settings = Settings::new();
        settings.windows = vec![
            window("user", None, None, Some(500_000)),
            window("user", None, Some("claude-opus-4-5"), Some(200_000)),
            window("project", Some("other"), None, Some(100_000)),
        ];
        settings.auto_compact_enabled = vec![SwitchSetting {
            file: "user.json".into(),
            scope: "user",
            project: None,
            enabled: false,
        }];
        let r = analyse(&dataset(requests, compactions), None, Some(settings), true);
        let c = &r.settings_comparison[0];
        // The model entry beats the general one; another project's file does not apply.
        assert_eq!(c.setting.as_ref().unwrap().tokens, Some(200_000));
        assert_eq!(c.fit, "within");
        assert_eq!(c.median_share_of_window_pct, Some(95.0));
        assert_eq!(c.above_window, 0);
        assert_eq!(c.auto_compact_enabled, Some(false));
        assert!(c.message.contains("not claimed for the past"));
        assert!(
            r.settings
                .as_ref()
                .unwrap()
                .as_of
                .contains("not the configuration of any past")
        );
    }

    #[test]
    fn no_setting_auto_value_and_other_agents_stay_visible() {
        let build = || {
            (
                (0..10).map(|t| req("a", "s", "m", t * 1000)).collect(),
                (0..3)
                    .map(|i| comp("a", "s", 500 + i * 2000, 90_000, Some(true)))
                    .collect(),
            )
        };
        let (rq, cp) = build();
        let r = analyse(&dataset(rq, cp), None, Some(Settings::new()), true);
        assert_eq!(r.settings_comparison[0].fit, "no_setting");
        let (rq, cp) = build();
        let mut s = Settings::new();
        s.windows = vec![window("user", None, None, None)];
        let r = analyse(&dataset(rq, cp), None, Some(s), true);
        assert_eq!(r.settings_comparison[0].fit, "auto_value");
        let (rq, cp) = build();
        let r = analyse(&dataset(rq, cp), None, None, true);
        assert!(r.settings.is_none() && r.settings_comparison.is_empty());
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("not read for this agent"))
        );
        let (rq, cp) = build();
        let mut s = Settings::new();
        s.windows = vec![window("user", None, None, Some(200_000))];
        let r = analyse(&dataset(rq, cp), None, Some(s), true);
        assert_eq!(r.settings_comparison[0].fit, "below");
    }

    #[test]
    fn window_values_are_numbers_or_auto() {
        use serde_json::json;
        assert_eq!(window_value(&json!("auto")), Some((None, None)));
        assert_eq!(window_value(&json!(200_000)), Some((Some(200_000), None)));
        assert!(window_value(&json!(50_000)).unwrap().1.is_some());
        assert!(window_value(&json!("big")).is_none());
        assert!(model_matches(
            "claude-opus-4-5",
            "claude-opus-4-5-20251101[1m]"
        ));
        assert!(!model_matches("claude-opus-4-5", "claude-sonnet-4-5"));
    }
}
