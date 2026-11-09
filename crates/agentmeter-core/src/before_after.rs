//! `--changed-on`: the periods before and after a change the user dated.
//!
//! This only chooses the two windows and hands them to [`compare`]; every figure, limitation
//! and reliability rule is that function's. The change point is given by the user, never
//! guessed. The later window runs from it to now, the earlier one is as long and ends at it, so
//! both cover the same number of calendar days. A later number is a correlation with whatever
//! else changed in that time, not the effect of the change.

use crate::compare::{Comparison, Window, compare};
use crate::model::Dataset;
use crate::time::format_iso;
use serde::Serialize;

const NOTICE: &str = "The date is the user's: Agent Meter does not know what changed. A \
difference between the periods is not an effect of the change; other tasks, models, front ends and \
the length of the periods change the numbers too (see the limitations).";

#[derive(Debug, Clone, Serialize)]
pub struct BeforeAfter {
    /// The day the change took effect, 00:00 UTC, as given.
    pub change_point: String,
    /// The user's own name for the change, if any.
    pub label: Option<String>,
    /// Calendar days of each period; zero when the change point is not in the past.
    pub period_days: i64,
    pub notice: &'static str,
    pub comparison: Comparison,
}

/// Compares the period before `change_ms` with the one from it to `now_ms`, of equal length.
pub fn before_after(
    dataset: &Dataset,
    change_ms: i64,
    label: Option<&str>,
    now_ms: i64,
) -> BeforeAfter {
    let len = (now_ms - change_ms).max(0);
    let later = Window {
        from_ms: change_ms,
        to_ms: change_ms + len,
    };
    let earlier = Window {
        from_ms: change_ms - len,
        to_ms: change_ms,
    };
    let comparison = compare(dataset, earlier, later);
    BeforeAfter {
        change_point: format_iso(change_ms),
        label: label.map(str::to_owned),
        period_days: comparison.later.days as i64,
        notice: NOTICE,
        comparison,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Request;
    use crate::time::DAY_MS;

    fn req(day: i64, i: i64) -> Request {
        Request {
            session: format!("s{day}"),
            project: "demo".into(),
            source: 0,
            message_id: String::new(),
            time_ms: day * DAY_MS + i * 1000,
            model: "m".into(),
            sub: false,
            fresh: 100,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            output: 1,
            window: None,
        }
    }

    fn data(days: std::ops::Range<i64>, per_day: i64) -> Dataset {
        Dataset {
            requests: days
                .flat_map(|d| (0..per_day).map(move |i| req(d, i)))
                .collect(),
            ..Dataset::default()
        }
    }

    #[test]
    fn periods_have_equal_length_around_the_change_point() {
        let b = before_after(
            &data(6..14, 50),
            10 * DAY_MS,
            Some("new rules"),
            14 * DAY_MS,
        );
        assert_eq!(b.period_days, 4);
        assert_eq!(b.label.as_deref(), Some("new rules"));
        assert_eq!(b.comparison.verdict, "compared");
        assert_eq!(b.comparison.earlier.to, b.comparison.later.from);
        assert_eq!(b.comparison.earlier.days, 4);
        assert_eq!(b.comparison.earlier.requests, 200);
    }

    #[test]
    fn missing_history_and_future_change_points_state_no_change() {
        // Nothing before the change point.
        let b = before_after(&data(10..14, 50), 10 * DAY_MS, None, 14 * DAY_MS);
        assert_eq!(b.comparison.verdict, "no_data");
        assert!(b.comparison.metrics.iter().all(|m| m.change.is_none()));
        // The change point is not in the past: both periods are empty.
        let b = before_after(&data(0..14, 50), 20 * DAY_MS, None, 14 * DAY_MS);
        assert_eq!(b.period_days, 0);
        assert_eq!(b.comparison.verdict, "no_data");
    }
}
