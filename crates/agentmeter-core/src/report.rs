//! Totals per period, day, session and project.

use crate::model::{Compaction, Request};
use crate::time::{format_day, format_iso};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

#[derive(Debug, Serialize)]
pub struct CompactionSummary {
    pub total: u64,
    pub auto: u64,
    pub manual: u64,
    /// Compactions whose trigger the log does not record.
    pub trigger_unknown: u64,
    pub avg_tokens_before: u64,
    pub min_tokens_before: u64,
    pub max_tokens_before: u64,
}

#[derive(Debug, Serialize)]
pub struct DayRow {
    pub date: String,
    pub requests: u64,
    pub input: u64,
    pub cache_read_pct: f64,
    pub compactions: u64,
}

#[derive(Debug, Serialize)]
pub struct ModelRow {
    pub model: String,
    pub requests: u64,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub requests: u64,
    pub sub_agent_requests: u64,
    pub sessions: u64,
    pub projects: u64,
    /// `fresh + cache_read + cache_write` over all requests.
    pub input: u64,
    pub fresh: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    pub cache_read_pct: f64,
    pub compactions: CompactionSummary,
    pub days: Vec<DayRow>,
    pub models: Vec<ModelRow>,
}

pub fn summarize(requests: &[&Request], compactions: &[&Compaction]) -> Summary {
    let mut summary = Summary {
        requests: requests.len() as u64,
        sub_agent_requests: 0,
        sessions: requests
            .iter()
            .map(|r| r.session.as_str())
            .collect::<HashSet<_>>()
            .len() as u64,
        projects: requests
            .iter()
            .map(|r| r.project.as_str())
            .collect::<HashSet<_>>()
            .len() as u64,
        input: 0,
        fresh: 0,
        cache_read: 0,
        cache_write_5m: 0,
        cache_write_1h: 0,
        output: 0,
        cache_read_pct: 0.0,
        compactions: CompactionSummary {
            total: compactions.len() as u64,
            auto: compactions.iter().filter(|c| c.auto == Some(true)).count() as u64,
            manual: compactions.iter().filter(|c| c.auto == Some(false)).count() as u64,
            trigger_unknown: compactions.iter().filter(|c| c.auto.is_none()).count() as u64,
            avg_tokens_before: 0,
            min_tokens_before: compactions
                .iter()
                .map(|c| c.tokens_before)
                .min()
                .unwrap_or(0),
            max_tokens_before: compactions
                .iter()
                .map(|c| c.tokens_before)
                .max()
                .unwrap_or(0),
        },
        days: Vec::new(),
        models: Vec::new(),
    };
    if !compactions.is_empty() {
        let sum: u64 = compactions.iter().map(|c| c.tokens_before).sum();
        summary.compactions.avg_tokens_before = sum / compactions.len() as u64;
    }

    // Per day: requests, input, cache read, compactions.
    let mut days: BTreeMap<String, [u64; 4]> = BTreeMap::new();
    let mut models: BTreeMap<&str, u64> = BTreeMap::new();
    for r in requests {
        summary.sub_agent_requests += u64::from(r.sub);
        summary.fresh += r.fresh;
        summary.cache_read += r.cache_read;
        summary.cache_write_5m += r.cache_write_5m;
        summary.cache_write_1h += r.cache_write_1h;
        summary.output += r.output;
        let day = days.entry(format_day(r.time_ms)).or_default();
        day[0] += 1;
        day[1] += r.context();
        day[2] += r.cache_read;
        *models.entry(r.model.as_str()).or_default() += 1;
    }
    for c in compactions {
        days.entry(format_day(c.time_ms)).or_default()[3] += 1;
    }
    summary.input =
        summary.fresh + summary.cache_read + summary.cache_write_5m + summary.cache_write_1h;
    summary.cache_read_pct = percent(summary.cache_read, summary.input);
    summary.days = days
        .into_iter()
        .map(
            |(date, [requests, input, cache_read, compactions])| DayRow {
                date,
                requests,
                input,
                cache_read_pct: percent(cache_read, input),
                compactions,
            },
        )
        .collect();
    let mut models: Vec<ModelRow> = models
        .into_iter()
        .map(|(model, requests)| ModelRow {
            model: model.to_owned(),
            requests,
        })
        .collect();
    models.sort_by(|a, b| b.requests.cmp(&a.requests).then(a.model.cmp(&b.model)));
    summary.models = models;
    summary
}

#[derive(Debug, Serialize)]
pub struct SessionRow {
    pub session: String,
    pub project: String,
    pub requests: u64,
    pub sub_agent_requests: u64,
    pub first: String,
    pub last: String,
    pub hours: f64,
    pub input: u64,
    pub peak_context: u64,
    pub cache_read_pct: f64,
    pub compactions: u64,
}

pub fn sessions(requests: &[&Request], compactions: &[&Compaction]) -> Vec<SessionRow> {
    #[derive(Default)]
    struct Acc<'a> {
        project: &'a str,
        requests: u64,
        sub: u64,
        first: i64,
        last: i64,
        input: u64,
        cache_read: u64,
        peak: u64,
    }
    let mut map: HashMap<&str, Acc> = HashMap::new();
    for r in requests {
        let acc = map.entry(r.session.as_str()).or_insert_with(|| Acc {
            project: r.project.as_str(),
            first: i64::MAX,
            last: i64::MIN,
            ..Acc::default()
        });
        acc.requests += 1;
        acc.sub += u64::from(r.sub);
        acc.first = acc.first.min(r.time_ms);
        acc.last = acc.last.max(r.time_ms);
        acc.input += r.context();
        acc.cache_read += r.cache_read;
        if !r.sub {
            acc.peak = acc.peak.max(r.context());
        }
    }
    let mut compacted: HashMap<&str, u64> = HashMap::new();
    for c in compactions {
        *compacted.entry(c.session.as_str()).or_default() += 1;
    }
    let mut rows: Vec<SessionRow> = map
        .into_iter()
        .map(|(session, acc)| SessionRow {
            session: session.to_owned(),
            project: acc.project.to_owned(),
            requests: acc.requests,
            sub_agent_requests: acc.sub,
            first: format_iso(acc.first),
            last: format_iso(acc.last),
            hours: (acc.last - acc.first) as f64 / 3_600_000.0,
            input: acc.input,
            peak_context: acc.peak,
            cache_read_pct: percent(acc.cache_read, acc.input),
            compactions: compacted.get(session).copied().unwrap_or(0),
        })
        .collect();
    rows.sort_by(|a, b| b.input.cmp(&a.input).then(a.session.cmp(&b.session)));
    rows
}

#[derive(Debug, Serialize)]
pub struct ProjectRow {
    pub project: String,
    pub sessions: u64,
    pub requests: u64,
    pub input: u64,
    pub input_share_pct: f64,
    pub cache_read_pct: f64,
}

pub fn projects(requests: &[&Request]) -> Vec<ProjectRow> {
    #[derive(Default)]
    struct Acc<'a> {
        sessions: HashSet<&'a str>,
        requests: u64,
        input: u64,
        cache_read: u64,
    }
    let mut map: HashMap<&str, Acc> = HashMap::new();
    let mut total = 0;
    for r in requests {
        let acc = map.entry(r.project.as_str()).or_default();
        acc.sessions.insert(r.session.as_str());
        acc.requests += 1;
        acc.input += r.context();
        acc.cache_read += r.cache_read;
        total += r.context();
    }
    let mut rows: Vec<ProjectRow> = map
        .into_iter()
        .map(|(project, acc)| ProjectRow {
            project: project.to_owned(),
            sessions: acc.sessions.len() as u64,
            requests: acc.requests,
            input: acc.input,
            input_share_pct: percent(acc.input, total),
            cache_read_pct: percent(acc.cache_read, acc.input),
        })
        .collect();
    rows.sort_by(|a, b| b.input.cmp(&a.input).then(a.project.cmp(&b.project)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(session: &str, time_ms: i64, fresh: u64, read: u64, write: u64) -> Request {
        Request {
            session: session.into(),
            project: "demo".into(),
            source: 0,
            message_id: String::new(),
            time_ms,
            model: "m".into(),
            sub: false,
            fresh,
            cache_read: read,
            cache_write_5m: write,
            cache_write_1h: 0,
            output: 1,
            window: None,
        }
    }

    #[test]
    fn summary_adds_up() {
        let a = request("a", 0, 1, 90, 9);
        let b = request("b", 86_400_000, 0, 100, 0);
        let summary = summarize(&[&a, &b], &[]);
        assert_eq!(summary.input, 200);
        assert_eq!(summary.sessions, 2);
        assert_eq!(summary.days.len(), 2);
        assert!((summary.cache_read_pct - 95.0).abs() < 1e-9);
    }
}
