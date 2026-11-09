//! Captures: the aggregated figures of one period, kept so a later period can be compared with
//! them. A capture is made on request only and holds numbers, never conversation, source code or
//! configuration text: the window facts and figures of [`crate::compare`], the source, the
//! project filter, the rule versions and a few counts on data quality.
//!
//! This module has no I/O. Two captures are compared by the same function as two windows of the
//! logs ([`compare_sides`]); when they were made under different rules, for another source or
//! project, nothing is compared and the reason is given. Values from `--probe` are not part of a
//! capture.

use crate::compare::{
    Comparison, Figure, Limitation, METRICS, Side, Window, WindowFacts, compare_sides, side,
};
use crate::findings::{BASELINE_VERSION, REBUILD_VERSION};
use crate::model::Dataset;
use crate::time::format_iso;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version of the file format; a file with another number is not read.
pub const SCHEMA_VERSION: u32 = 1;

/// What the logs of the source looked like when the capture was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureQuality {
    pub files: u64,
    pub skipped_lines: u64,
    pub files_without_usage: u64,
    pub compressed_files_unreadable: u64,
    pub analysable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedFigure {
    pub name: String,
    pub value: Option<f64>,
    pub denominator: u64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    pub schema_version: u32,
    /// The name the capture is stored and found under.
    pub name: String,
    /// The user's own name for a change, if given.
    pub label: Option<String>,
    pub created: String,
    /// `claude` or `codex`.
    pub tool: String,
    /// The project filter in effect, `None` for all projects.
    pub project: Option<String>,
    /// Version of each rule whose figure is part of the capture.
    pub rules: BTreeMap<String, u32>,
    pub data_quality: CaptureQuality,
    pub facts: WindowFacts,
    pub figures: Vec<CapturedFigure>,
}

/// Names are file names: letters, digits, `-`, `_` and `.`, not starting with a dot.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn rules() -> BTreeMap<String, u32> {
    BTreeMap::from([
        ("baseline-load".to_owned(), BASELINE_VERSION),
        ("cache-rebuild".to_owned(), REBUILD_VERSION),
    ])
}

/// Measures `window` of `dataset` (already limited to the project, if any) into a capture.
pub fn capture(
    dataset: &Dataset,
    tool: &str,
    project: Option<&str>,
    name: &str,
    label: Option<&str>,
    window: Window,
    now_ms: i64,
) -> Capture {
    let measured = side(dataset, window);
    let stats = &dataset.stats;
    Capture {
        schema_version: SCHEMA_VERSION,
        name: name.to_owned(),
        label: label.map(str::to_owned),
        created: format_iso(now_ms),
        tool: tool.to_owned(),
        project: project.map(str::to_owned),
        rules: rules(),
        data_quality: CaptureQuality {
            files: stats.files,
            skipped_lines: stats.skipped_lines,
            files_without_usage: stats.files_without_usage,
            compressed_files_unreadable: stats.compressed_files_unreadable,
            analysable: dataset.quality().analysable,
        },
        facts: measured.facts,
        figures: measured
            .figures
            .into_iter()
            .map(|(name, f)| CapturedFigure {
                name: name.to_owned(),
                value: f.value,
                denominator: f.denominator,
                note: f.note,
            })
            .collect(),
    }
}

/// Reads a capture file; a damaged file or another schema version is an error with the reason.
pub fn parse(text: &str) -> Result<Capture, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not a capture file: {e}"))?;
    match value.get("schema_version").and_then(|v| v.as_u64()) {
        Some(v) if v == u64::from(SCHEMA_VERSION) => {}
        Some(v) => {
            return Err(format!(
                "schema version {v}, this version reads {SCHEMA_VERSION}"
            ));
        }
        None => return Err("no schema_version".to_owned()),
    }
    let capture: Capture =
        serde_json::from_value(value).map_err(|e| format!("not a capture file: {e}"))?;
    for (name, _, _) in METRICS {
        if !capture.figures.iter().any(|f| f.name == name) {
            return Err(format!("the figure `{name}` is missing"));
        }
    }
    Ok(capture)
}

/// Which capture is meant, for the result.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureRef {
    pub name: String,
    pub label: Option<String>,
    pub created: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureComparison {
    /// False when the captures cannot be compared; `reason` says why and no figure is given.
    pub comparable: bool,
    pub reason: Option<String>,
    pub earlier: CaptureRef,
    pub later: CaptureRef,
    pub comparison: Option<Comparison>,
}

fn reference(c: &Capture) -> CaptureRef {
    CaptureRef {
        name: c.name.clone(),
        label: c.label.clone(),
        created: c.created.clone(),
    }
}

fn restore(c: &Capture) -> Side {
    let figures = METRICS
        .iter()
        .map(|(name, _, _)| {
            let f = c.figures.iter().find(|f| f.name == *name);
            (
                *name,
                Figure {
                    value: f.and_then(|f| f.value),
                    denominator: f.map_or(0, |f| f.denominator),
                    note: f.and_then(|f| f.note.clone()),
                },
            )
        })
        .collect();
    Side {
        facts: c.facts.clone(),
        figures,
    }
}

fn mismatch(a: &Capture, b: &Capture) -> Option<String> {
    if a.tool != b.tool {
        return Some(format!(
            "the captures are from different sources ({} and {})",
            a.tool, b.tool
        ));
    }
    if a.project != b.project {
        let show = |p: &Option<String>| p.clone().unwrap_or_else(|| "all projects".to_owned());
        return Some(format!(
            "the captures cover different projects ({} and {})",
            show(&a.project),
            show(&b.project)
        ));
    }
    if a.rules != b.rules {
        return Some(
            "the captures were made with different rule versions; their figures are not measured \
             the same way"
                .to_owned(),
        );
    }
    None
}

/// Compares two captures with the rules of [`crate::compare`]; `earlier` and `later` are names.
pub fn compare_captures(earlier: &Capture, later: &Capture) -> CaptureComparison {
    let reason = mismatch(earlier, later);
    let comparison = reason.is_none().then(|| {
        let mut comparison = compare_sides(restore(earlier), restore(later));
        // ISO timestamps of one format order as text.
        if earlier.facts.to > later.facts.from && later.facts.to > earlier.facts.from {
            comparison.limitations.push(Limitation {
                kind: "windows-overlap",
                text: "the periods of the two captures overlap; a request in both counts in both"
                    .to_owned(),
            });
        }
        comparison
    });
    CaptureComparison {
        comparable: reason.is_none(),
        reason,
        earlier: reference(earlier),
        later: reference(later),
        comparison,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Request;
    use crate::time::DAY_MS;

    fn data(first: i64, days: i64, per_day: i64, read: u64) -> Dataset {
        Dataset {
            requests: (first..first + days)
                .flat_map(|d| {
                    (0..per_day).map(move |i| Request {
                        session: format!("s{d}"),
                        project: "demo".into(),
                        source: 0,
                        message_id: String::new(),
                        time_ms: d * DAY_MS + i * 1000,
                        model: "m".into(),
                        sub: false,
                        fresh: 100 - read,
                        cache_read: read,
                        cache_write_5m: 0,
                        cache_write_1h: 0,
                        output: 1,
                        window: None,
                    })
                })
                .collect(),
            ..Dataset::default()
        }
    }

    fn win(first: i64) -> Window {
        Window {
            from_ms: first * DAY_MS,
            to_ms: (first + 4) * DAY_MS,
        }
    }

    fn make(name: &str, first: i64, per_day: i64, read: u64) -> Capture {
        let d = data(first, 4, per_day, read);
        capture(&d, "claude", None, name, None, win(first), 0)
    }

    #[test]
    fn two_captures_give_changes_and_denominators_like_compare() {
        let (a, b) = (make("before", 0, 50, 50), make("after", 10, 100, 80));
        let c = compare_captures(&a, &b);
        assert!(c.comparable && c.reason.is_none());
        let cmp = c.comparison.unwrap();
        assert_eq!(cmp.verdict, "compared");
        assert!(cmp.limitations.is_empty());
        let m = |n: &str| cmp.metrics.iter().find(|m| m.name == n).unwrap().clone();
        assert_eq!(m("requests_per_active_day").change_pct, Some(100.0));
        assert_eq!(m("cache_read_pct").change, Some(30.0));
        assert_eq!(m("cache_read_pct").denominator_earlier, 200 * 100);
        // The same as comparing the logs directly.
        let both = Dataset {
            requests: [data(0, 4, 50, 50).requests, data(10, 4, 100, 80).requests].concat(),
            ..Dataset::default()
        };
        let direct = crate::compare::compare(&both, win(0), win(10));
        assert_eq!(
            serde_json::to_value(&direct.metrics).unwrap(),
            serde_json::to_value(&cmp.metrics).unwrap()
        );
    }

    #[test]
    fn a_capture_survives_its_file_and_bad_files_say_why() {
        let a = make("before", 0, 50, 50);
        let text = serde_json::to_string(&a).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), text);
        let other = text.replace("\"schema_version\":1", "\"schema_version\":2");
        assert!(parse(&other).unwrap_err().contains("schema version 2"));
        assert!(parse("{}").unwrap_err().contains("schema_version"));
        assert!(parse("nope").is_err());
    }

    #[test]
    fn captures_that_do_not_match_are_not_compared() {
        let a = make("before", 0, 50, 50);
        let mut b = make("after", 10, 50, 50);
        b.rules.insert("baseline-load".into(), 99);
        let c = compare_captures(&a, &b);
        assert!(!c.comparable && c.comparison.is_none());
        assert!(c.reason.unwrap().contains("rule versions"));
        let mut b = make("after", 10, 50, 50);
        b.tool = "codex".into();
        assert!(compare_captures(&a, &b).reason.unwrap().contains("sources"));
        let mut b = make("after", 10, 50, 50);
        b.project = Some("x".into());
        assert!(
            compare_captures(&a, &b)
                .reason
                .unwrap()
                .contains("projects")
        );
    }

    #[test]
    fn empty_period_and_zero_denominators_stay_unknown() {
        let empty = capture(&Dataset::default(), "claude", None, "none", None, win(0), 0);
        let c = compare_captures(&empty, &make("after", 10, 50, 50));
        let cmp = c.comparison.unwrap();
        assert_eq!(cmp.verdict, "no_data");
        assert!(cmp.metrics.iter().all(|m| m.change.is_none()));
    }

    #[test]
    fn overlapping_periods_are_named() {
        let (a, b) = (make("a", 0, 50, 50), make("b", 2, 50, 50));
        let cmp = compare_captures(&a, &b).comparison.unwrap();
        assert_eq!(cmp.limitations[0].kind, "windows-overlap");
    }

    #[test]
    fn names_are_file_names() {
        assert!(valid_name("before-2026.10_a"));
        for bad in ["", ".hidden", "a/b", "a b", "../x"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
