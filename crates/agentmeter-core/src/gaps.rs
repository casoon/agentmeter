//! Gaps in the observation period and the retention of the transcripts.
//!
//! Both only use what is already read: the days with requests, and (for the retention) the
//! setting a caller read. A day without requests between the first and the last observed day may
//! be a pause or lost logs; nothing here says which. The hint that logs may have been deleted
//! early is a comparison of two numbers with their basis, never a proof.

use crate::compaction::SettingsFile;
use crate::time::{DAY_MS, format_day, format_iso};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Share of the retention the oldest entry has to be old for the early-loss hint. Uncalibrated
/// assumption.
const NEAR_RETENTION: f64 = 0.9;
/// Days past the retention the oldest entry may be (the sweep does not run at midnight) and still
/// count as "at the retention". Uncalibrated assumption.
const SLACK_DAYS: f64 = 1.0;

/// Claude Code deletes transcripts after this many days when no settings file says otherwise.
pub const CLAUDE_DEFAULT_DAYS: u64 = 30;

pub const GAP_NOTE: &str = "A day without requests can be a pause or missing logs; the logs do \
not say which.";

/// A run of UTC days without requests, both ends included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapPeriod {
    pub from: String,
    pub to: String,
    pub days: u64,
}

/// Days without requests between the first and the last day with requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gaps {
    pub first_day: String,
    pub last_day: String,
    /// UTC calendar days from the first to the last day, both included.
    pub span_days: u64,
    pub active_days: u64,
    /// Days in the span without a request; `span_days` minus `active_days`.
    pub gap_days: u64,
    pub longest_gap: Option<GapPeriod>,
    /// Every run of gap days, in time order.
    pub periods: Vec<GapPeriod>,
}

/// The gaps of a set of request times; `None` when there is no request, since "no gap" would be
/// a claim about data that is not there.
pub fn gaps(times_ms: impl IntoIterator<Item = i64>) -> Option<Gaps> {
    let days: BTreeSet<i64> = times_ms.into_iter().map(|t| t.div_euclid(DAY_MS)).collect();
    let (first, last) = (*days.first()?, *days.last()?);
    let mut periods: Vec<GapPeriod> = Vec::new();
    let mut previous = first;
    for &day in days.iter().skip(1) {
        if day > previous + 1 {
            periods.push(GapPeriod {
                from: format_day((previous + 1) * DAY_MS),
                to: format_day((day - 1) * DAY_MS),
                days: (day - previous - 1) as u64,
            });
        }
        previous = day;
    }
    let gap_days = periods.iter().map(|p| p.days).sum();
    // The first of equally long runs.
    let longest_gap = periods
        .iter()
        .fold(None::<&GapPeriod>, |best, p| match best {
            Some(b) if b.days >= p.days => Some(b),
            _ => Some(p),
        })
        .cloned();
    Some(Gaps {
        first_day: format_day(first * DAY_MS),
        last_day: format_day(last * DAY_MS),
        span_days: (last - first + 1) as u64,
        active_days: days.len() as u64,
        gap_days,
        longest_gap,
        periods,
    })
}

/// A value of `cleanupPeriodDays` found in a settings file.
#[derive(Debug, Clone, Serialize)]
pub struct RetentionValue {
    pub file: String,
    pub scope: &'static str,
    pub project: Option<String>,
    /// `None` when the value is not a whole number of at least 1 (Claude Code rejects it).
    pub days: Option<u64>,
}

/// The comparison of the oldest entry with the retention. A hint, not a proof.
#[derive(Debug, Clone, Serialize)]
pub struct RetentionHint {
    /// The oldest request, ISO 8601 UTC.
    pub oldest_entry: String,
    /// Age of the oldest request in days at the time of the run.
    pub age_days: f64,
    /// The retention the age is compared with.
    pub retention_days: u64,
    /// Share of the retention the oldest entry has reached (age / retention).
    pub age_of_retention_pct: f64,
    /// `low` or `medium`; never `high`: an early start of the logs has other causes too.
    pub certainty: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Retention {
    /// False when the source has no such setting (Codex); then nothing else is filled.
    pub measurable: bool,
    pub key: &'static str,
    pub reference: &'static str,
    pub default_days: Option<u64>,
    pub files: Vec<SettingsFile>,
    pub values: Vec<RetentionValue>,
    /// Projects whose working directory is unknown or gone: their settings are unknown.
    pub projects_not_read: Vec<String>,
    /// The retention used for the comparison: the shortest value found, else the default.
    /// `None` when the setting is unusable or the source has none.
    pub effective_days: Option<u64>,
    /// `settings` or `default`.
    pub basis: Option<&'static str>,
    pub note: String,
    pub hint: Option<RetentionHint>,
}

const REFERENCE: &str = "Claude Code settings reference, `cleanupPeriodDays` (checked 2026-11): \
whole number of days, at least 1, default 30; valid in any settings file; transcripts older than \
that are deleted by a background sweep after a session starts. The documentation gives no \
version for this key, so the current documentation is the reference.";

/// The retention of a source that has none.
pub fn not_measurable(reason: &str) -> Retention {
    Retention {
        measurable: false,
        key: "cleanupPeriodDays",
        reference: REFERENCE,
        default_days: None,
        files: Vec::new(),
        values: Vec::new(),
        projects_not_read: Vec::new(),
        effective_days: None,
        basis: None,
        note: reason.to_owned(),
        hint: None,
    }
}

/// Retention from what the settings files hold. Compares the oldest request with it.
pub fn retention(
    files: Vec<SettingsFile>,
    values: Vec<RetentionValue>,
    projects_not_read: Vec<String>,
    oldest_ms: Option<i64>,
    now_ms: i64,
) -> Retention {
    let usable: Vec<u64> = values.iter().filter_map(|v| v.days).collect();
    let unusable = values.iter().any(|v| v.days.is_none());
    let (effective_days, basis, note) = if unusable && usable.is_empty() {
        (
            None,
            None,
            "a settings file holds a `cleanupPeriodDays` that is not a whole number of at least 1; \
             the retention in effect is not known"
                .to_owned(),
        )
    } else if let Some(min) = usable.iter().min() {
        (
            Some(*min),
            Some("settings"),
            if usable.iter().any(|d| d != min) {
                "several values found; the shortest is used, since the sweep of any session may \
                 apply it"
                    .to_owned()
            } else {
                "set in the settings files read".to_owned()
            },
        )
    } else {
        (
            Some(CLAUDE_DEFAULT_DAYS),
            Some("default"),
            "no settings file read sets it; the documented default applies unless a source not \
             read (managed settings, command line) sets it"
                .to_owned(),
        )
    };
    let all_known = projects_not_read.is_empty()
        && !unusable
        && files.iter().all(|f| matches!(f.state, "read" | "missing"));
    let hint = match (effective_days, oldest_ms) {
        (Some(days), Some(oldest)) => hint(days, oldest, now_ms, all_known),
        _ => None,
    };
    Retention {
        measurable: true,
        key: "cleanupPeriodDays",
        reference: REFERENCE,
        default_days: Some(CLAUDE_DEFAULT_DAYS),
        files,
        values,
        projects_not_read,
        effective_days,
        basis,
        note,
        hint,
    }
}

fn hint(
    retention_days: u64,
    oldest_ms: i64,
    now_ms: i64,
    all_known: bool,
) -> Option<RetentionHint> {
    let age_days = (now_ms - oldest_ms) as f64 / DAY_MS as f64;
    let retention = retention_days as f64;
    if age_days < retention * NEAR_RETENTION || age_days > retention + SLACK_DAYS {
        return None;
    }
    let certainty = if all_known { "medium" } else { "low" };
    Some(RetentionHint {
        oldest_entry: format_iso(oldest_ms),
        age_days: (age_days * 10.0).round() / 10.0,
        retention_days,
        age_of_retention_pct: (age_days * 1000.0 / retention).round() / 10.0,
        certainty,
        text: format!(
            "the oldest request is {age_days:.1} days old and the retention is {retention_days} \
             days; earlier logs may have been deleted by Claude Code. This is a hint, not proof: \
             the logs may also simply start there. Certainty: {certainty}"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: i64) -> i64 {
        d * DAY_MS + 3_600_000
    }

    #[test]
    fn finds_runs_of_days_without_requests() {
        // Days 0,1, 4, 8,9: gaps 2-3 (2 days) and 5-7 (3 days).
        let g = gaps([day(0), day(1), day(1), day(4), day(8), day(9)]).unwrap();
        assert_eq!((g.span_days, g.active_days, g.gap_days), (10, 5, 5));
        assert_eq!(g.periods.len(), 2);
        assert_eq!(g.periods[0].from, "1970-01-03");
        assert_eq!(g.periods[0].to, "1970-01-04");
        let longest = g.longest_gap.unwrap();
        assert_eq!((longest.from.as_str(), longest.days), ("1970-01-06", 3));
    }

    #[test]
    fn no_gap_one_day_and_no_data() {
        let g = gaps([day(5), day(6), day(7)]).unwrap();
        assert_eq!((g.gap_days, g.longest_gap, g.periods.len()), (0, None, 0));
        let g = gaps([day(5)]).unwrap();
        assert_eq!((g.span_days, g.active_days, g.gap_days), (1, 1, 0));
        assert!(gaps([]).is_none());
    }

    fn file(state: &'static str) -> SettingsFile {
        SettingsFile {
            path: "s.json".into(),
            scope: "user",
            project: None,
            state,
        }
    }

    fn plain(oldest_days_ago: i64, now: i64) -> Retention {
        retention(
            vec![],
            vec![],
            vec![],
            Some(now - oldest_days_ago * DAY_MS),
            now,
        )
    }

    #[test]
    fn hint_only_near_the_retention_and_states_its_basis() {
        let now = 100 * DAY_MS;
        let r = retention(
            vec![file("missing")],
            vec![],
            vec![],
            Some(now - 29 * DAY_MS),
            now,
        );
        assert_eq!((r.effective_days, r.basis), (Some(30), Some("default")));
        let h = r.hint.unwrap();
        assert_eq!((h.retention_days, h.certainty), (30, "medium"));
        assert!(h.text.contains("hint, not proof"));
        // Far younger, and older than the retention: no hint.
        assert!(plain(10, now).hint.is_none());
        assert!(plain(60, now).hint.is_none());
    }

    #[test]
    fn settings_win_over_default_and_unknowns_stay_visible() {
        let now = 100 * DAY_MS;
        let value = |days| RetentionValue {
            file: "s.json".into(),
            scope: "user",
            project: None,
            days,
        };
        let r = retention(
            vec![file("read")],
            vec![value(Some(14)), value(Some(90))],
            vec![],
            Some(now - 14 * DAY_MS),
            now,
        );
        assert_eq!((r.effective_days, r.basis), (Some(14), Some("settings")));
        assert!(r.hint.is_some());
        // An unusable value: the retention is not known, no hint, no default invented.
        let r = retention(
            vec![file("read")],
            vec![value(None)],
            vec![],
            Some(now - 29 * DAY_MS),
            now,
        );
        assert_eq!(r.effective_days, None);
        assert!(r.hint.is_none());
        // An unreadable file lowers the certainty.
        let r = retention(
            vec![file("invalid_json")],
            vec![],
            vec![],
            Some(now - 30 * DAY_MS),
            now,
        );
        assert_eq!(r.hint.unwrap().certainty, "low");
    }
}
