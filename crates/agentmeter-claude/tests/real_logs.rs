//! Optional check against the maintainer's real logs. Runs only with `AGENTMETER_REAL_LOGS=1`
//! and otherwise passes without doing anything. The logs keep evolving, so the days of the
//! acceptance table (`plan/referenz/00-konzept.md` section 6, counted in local time there, in UTC
//! here) are compared with a tolerance and the totals are printed.

use agentmeter_claude::{config_dirs, read};
use agentmeter_core::report::summarize;

/// Day, requests, input in millions, compactions (acceptance values, 2026-10-29 noon).
const ACCEPTANCE: &[(&str, f64, f64, f64)] = &[
    ("2026-10-23", 3576.0, 739.0, 0.0),
    ("2026-10-24", 4344.0, 1169.0, 1.0),
    ("2026-10-25", 866.0, 274.0, 1.0),
    ("2026-10-26", 8927.0, 2198.0, 4.0),
    ("2026-10-27", 7299.0, 2021.0, 8.0),
    ("2026-10-28", 2809.0, 898.0, 4.0),
];
/// Relative tolerance: days shift with the time zone and the logs keep changing.
const TOLERANCE: f64 = 0.10;

#[test]
fn real_logs_match_the_acceptance_values() {
    if std::env::var_os("AGENTMETER_REAL_LOGS").is_none_or(|v| v != "1") {
        eprintln!("skipped: set AGENTMETER_REAL_LOGS=1 to run against real logs");
        return;
    }
    let dataset = read(&config_dirs());
    assert!(dataset.stats.files > 0, "no real logs found");
    let requests: Vec<_> = dataset.requests.iter().collect();
    let compactions: Vec<_> = dataset.compactions.iter().collect();
    let summary = summarize(&requests, &compactions);
    println!(
        "total: {} requests, input {} M, {} compactions, {} sessions",
        summary.requests,
        summary.input / 1_000_000,
        summary.compactions.total,
        summary.sessions
    );
    for (date, want_requests, want_input_m, want_compactions) in ACCEPTANCE {
        let day = summary.days.iter().find(|d| d.date == *date).unwrap();
        let input_m = day.input as f64 / 1e6;
        println!(
            "{date}: {} requests (want {want_requests}), {input_m:.0} M (want {want_input_m}), {} compactions (want {want_compactions})",
            day.requests, day.compactions
        );
        assert!((day.requests as f64 - want_requests).abs() <= want_requests * TOLERANCE);
        assert!((input_m - want_input_m).abs() <= want_input_m * TOLERANCE);
        assert!(
            (day.compactions as f64 - want_compactions).abs() <= 1.0 + want_compactions * TOLERANCE
        );
    }
}
