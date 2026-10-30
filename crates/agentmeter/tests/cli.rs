//! The binary against the synthetic fixtures.

use std::path::Path;
use std::process::{Command, Output};

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Runs with `claude` as the Claude config dir and no Codex home (a directory that does not exist).
fn run(claude: &str, args: &[&str]) -> Output {
    run_both(claude, "does-not-exist", args)
}

fn run_both(claude: &str, codex: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentmeter"))
        .args(args)
        .env("CLAUDE_CONFIG_DIR", fixtures().join(claude))
        .env("CODEX_HOME", fixtures().join(codex))
        // `--probe` never starts the real claude in a test.
        .env("AGENTMETER_CLAUDE", fixtures().join("probe/claude"))
        .env_remove("NO_COLOR")
        .output()
        .unwrap()
}

/// The first (and only) source of the JSON output.
fn json(fixture: &str, args: &[&str]) -> serde_json::Value {
    let output = run(fixture, args);
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        v["sources"].as_array().unwrap().len(),
        1,
        "only sources with data"
    );
    v["sources"][0].clone()
}

#[test]
fn json_summary_has_stable_fields_and_data_quality() {
    let v = json("epoch-current", &["--json"]);
    assert_eq!(v["tool"], "claude");
    assert_eq!(v["summary"]["requests"], 4);
    assert_eq!(v["summary"]["input"], 163_511);
    assert_eq!(v["summary"]["compactions"]["manual"], 1);
    assert_eq!(v["data_quality"]["analysable"], true);
    assert_eq!(v["data_quality"]["observed_from"], "2026-10-28T12:00:05Z");
    assert_eq!(v["data_quality"]["unknown_types"]["future-event"], 1);
    assert_eq!(v["data_quality"]["files_without_usage"], 0);
}

#[test]
fn findings_carry_rule_version_measure_evidence_and_next_step() {
    let v = json("epoch-current", &["--json", "--findings"]);
    assert_eq!(v["verdict"], "findings");
    let rebuild = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "cache-rebuild")
        .unwrap();
    assert_eq!(rebuild["rule_version"], 1);
    assert_eq!(rebuild["level"], "info");
    assert_eq!(
        rebuild["values"]["expected"], 1,
        "the rebuild follows a compaction"
    );
    assert_eq!(rebuild["evidence"][0]["event"], "msg_M3");
    assert!(rebuild["measure"]["denominator"].as_u64().unwrap() >= 1);
    assert!(!rebuild["next_step"].as_str().unwrap().is_empty());
    assert!(!rebuild["counterexamples"].as_array().unwrap().is_empty());
    // One clean session start is too few for baseline-load: reported as not measurable.
    let baseline = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rule"] == "baseline-load")
        .unwrap();
    assert_eq!(baseline["status"], "not_measurable");
}

#[test]
fn no_data_is_not_reported_as_no_findings() {
    let v = json(
        "epoch-current",
        &["--json", "--findings", "--since", "2099-01-01"],
    );
    assert_eq!(v["verdict"], "no_data");
    assert_eq!(
        v["data_quality"]["analysable"], true,
        "the logs themselves have usage"
    );
    let text =
        String::from_utf8(run("epoch-current", &["--findings", "--since", "2099-01-01"]).stdout)
            .unwrap();
    assert!(text.contains("No analysis possible"));
    assert!(!text.contains("No findings"));
}

#[test]
fn inherited_history_start_is_excluded_from_baseline_without_cause_claims() {
    let text = String::from_utf8(run("epoch-resumed", &["--findings"]).stdout).unwrap();
    assert!(text.contains("1 files without usage"));
    assert!(!text.contains("CLAUDE.md"));
}

#[test]
fn text_has_no_color_codes_when_piped() {
    let text =
        String::from_utf8(run("epoch-current", &["--summary", "--projects", "--sessions"]).stdout)
            .unwrap();
    assert!(!text.contains('\x1b'));
    assert!(text.contains("demo-app"));
}

#[test]
fn text_lines_fit_88_columns() {
    for (claude, codex) in [
        ("epoch-current", "codex-basic"),
        ("epoch-baseline", "codex-subagent"),
        ("epoch-projects", "codex-compacted"),
    ] {
        let output = run_both(
            claude,
            codex,
            &["--summary", "--sessions", "--projects", "--probe"],
        );
        assert!(output.status.success());
        for line in String::from_utf8(output.stdout).unwrap().lines() {
            assert!(line.chars().count() <= 88, "{claude}: {line}");
        }
    }
}

#[test]
fn bad_flag_exits_two_and_missing_logs_exit_one() {
    assert_eq!(run("epoch-current", &["--nope"]).status.code(), Some(2));
    assert_eq!(run("does-not-exist", &[]).status.code(), Some(1));
}

#[test]
fn project_without_a_match_exits_one() {
    let out = run("epoch-current", &["--project", "no-such-project"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no project matches"));
    assert!(
        run("epoch-current", &["--project", "demo"])
            .status
            .success()
    );
}

#[test]
fn sources_are_reported_separately_with_their_own_data_quality() {
    let output = run_both(
        "epoch-current",
        "codex-no-token-count",
        &["--json", "--findings"],
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let sources = v["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(
        (sources[0]["tool"].as_str(), sources[1]["tool"].as_str()),
        (Some("claude"), Some("codex"))
    );
    // Codex: a file without token_count and a compressed file; nothing to analyse.
    let codex = &sources[1];
    assert_eq!(codex["verdict"], "no_data");
    assert_eq!(codex["data_quality"]["files_without_usage"], 1);
    assert_eq!(codex["data_quality"]["compressed_files_unreadable"], 1);
    assert_eq!(codex["data_quality"]["analysable"], false);
    assert_eq!(sources[0]["data_quality"]["compressed_files_unreadable"], 0);
}

#[test]
fn tool_codex_reads_only_codex_and_marks_what_it_cannot_measure() {
    let output = run_both(
        "epoch-current",
        "codex-basic",
        &["--tool", "codex", "--json", "--findings", "--summary"],
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["sources"].as_array().unwrap().len(), 1);
    let codex = &v["sources"][0];
    assert_eq!(codex["tool"], "codex");
    assert_eq!(codex["summary"]["requests"], 2);
    assert_eq!(codex["summary"]["input"], 3500);
    assert_eq!(codex["data_quality"]["cache_write_measured"], false);
    let rule = |id: &str| {
        codex["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["rule"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(rule("cache-rebuild")["status"], "not_measurable");
    assert!(
        rule("cache-rebuild")["reason"]
            .as_str()
            .unwrap()
            .contains("no cache write data")
    );
    let text = String::from_utf8(
        run_both(
            "epoch-current",
            "codex-basic",
            &["--tool", "codex", "--findings"],
        )
        .stdout,
    )
    .unwrap();
    assert!(text.contains("Codex CLI"));
    assert!(text.contains("cache writes not recorded"));
}

#[test]
fn explicit_tool_without_logs_fails_but_all_skips_empty_sources() {
    let out = run_both("epoch-current", "does-not-exist", &["--tool", "codex"]);
    assert_eq!(out.status.code(), Some(1));
    let out = run_both("epoch-current", "does-not-exist", &["--json"]);
    assert!(out.status.success());
    assert_eq!(
        run_both("epoch-current", "x", &["--tool", "nope"])
            .status
            .code(),
        Some(2)
    );
}

/// Field names documented under each `### \`path\`` heading of docs/reference/json.md.
fn documented_fields() -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/json.md");
    let text = std::fs::read_to_string(path).unwrap();
    let mut map = std::collections::BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("### `") {
            let name = rest.trim_end_matches('`').to_owned();
            map.insert(name.clone(), std::collections::BTreeSet::new());
            current = Some(name);
        } else if line.starts_with('#') {
            current = None;
        } else if let (Some(name), Some(item)) = (&current, line.strip_prefix("- `")) {
            let field = item.split('`').next().unwrap().to_owned();
            map.get_mut(name).unwrap().insert(field);
        }
    }
    map
}

/// The object at a documented path: `root`, or `sources[].x[].y` following the first element.
fn object_at<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path == "root" {
        return Some(root);
    }
    let mut value = root;
    for part in path.split('.') {
        let (name, array) = part.strip_suffix("[]").map_or((part, false), |n| (n, true));
        value = &value[name];
        if array {
            value = value.as_array()?.first()?;
        }
    }
    Some(value)
}

#[test]
fn documented_json_fields_equal_the_real_ones() {
    let output = run_both(
        "epoch-current",
        "codex-cache-write",
        &[
            "--json",
            "--summary",
            "--sessions",
            "--projects",
            "--findings",
            "--probe",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    let documented = documented_fields();
    assert!(
        documented.len() == 18,
        "the doc lists the objects: {documented:?}"
    );
    for (path, fields) in &documented {
        let Some(object) = object_at(&v, path) else {
            // `## --check` style headings are not in this output.
            panic!("documented path {path} is not in the output");
        };
        let actual: std::collections::BTreeSet<String> =
            object.as_object().unwrap().keys().cloned().collect();
        assert_eq!(&actual, fields, "fields of {path}");
    }
}

#[test]
fn check_json_fields_are_the_documented_ones() {
    let output = run("epoch-current", &["--check", "data-quality", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["check", "passed", "schema_version", "sources"]);
    assert_eq!(v["sources"][0]["passed"], true);
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/json.md"),
    )
    .unwrap();
    for key in keys.iter().chain(["tool", "reasons"].iter()) {
        assert!(doc.contains(&format!("`{key}`")), "{key} is documented");
    }
}

#[test]
fn check_exit_codes() {
    let ok = run("epoch-current", &["--check", "data-quality"]);
    assert_eq!(ok.status.code(), Some(0));
    assert!(
        String::from_utf8(ok.stdout)
            .unwrap()
            .contains("data quality ok")
    );
    // Codex with only a file without token_count and a damaged .zst: nothing to analyse.
    let empty = run_both(
        "epoch-current",
        "codex-no-token-count",
        &["--check", "data-quality"],
    );
    assert_eq!(empty.status.code(), Some(1));
    let text = String::from_utf8(empty.stdout).unwrap();
    assert!(text.contains("Codex CLI: data quality check failed") && text.contains("no requests"));
    // A period without requests fails like a source without usage.
    let none = run(
        "epoch-current",
        &["--check", "data-quality", "--since", "2099-01-01"],
    );
    assert_eq!(none.status.code(), Some(1));
    // Bad calls: exit 2 and the allowed values.
    let bad = run("epoch-current", &["--check", "findings"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(
        String::from_utf8(bad.stderr)
            .unwrap()
            .contains("data-quality")
    );
    assert_eq!(run("epoch-current", &["--check"]).status.code(), Some(2));
}

#[test]
fn findings_never_fail_the_check() {
    // epoch-current has notices and info; the check still passes.
    let v = json("epoch-current", &["--json", "--findings"]);
    assert!(!v["findings"].as_array().unwrap().is_empty());
    assert_eq!(
        run("epoch-current", &["--check", "data-quality"])
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn without_a_terminal_no_arguments_print_the_text_summary_not_the_menu() {
    // Under test stdout and stderr are pipes: the menu must not start.
    let output = run("epoch-current", &[]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("SUMMARY") && !text.contains('\x1b'));
}

#[test]
fn probe_splits_the_session_start_with_what_claude_reports() {
    let v = json("epoch-baseline", &["--json", "--probe"]);
    let probe = &v["probe"];
    assert_eq!(probe["runs"][0]["place"], "empty directory");
    // 3k + 19.1k + 1.4k + 9.9k; deferred tools and free space are not sent.
    assert_eq!(probe["runs"][0]["total"], 33_400);
    assert_eq!(probe["failures"].as_array().unwrap().len(), 0);
    let baseline = &v["findings"][0];
    assert_eq!(baseline["rule"], "baseline-load");
    assert_eq!(baseline["parts"][0]["value"], 33_400);
    let text = baseline["conclusion"]["text"].as_str().unwrap();
    // The front end the log names is not what /context in the CLI sees.
    assert!(text.contains("claude-desktop"), "{text}");
}

#[test]
fn a_probe_that_cannot_run_is_reported_not_fatal() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmeter"))
        .args(["--probe", "--json"])
        .env("CLAUDE_CONFIG_DIR", fixtures().join("epoch-baseline"))
        .env("CODEX_HOME", fixtures().join("does-not-exist"))
        .env("AGENTMETER_CLAUDE", fixtures().join("no-such-program"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let source = &v["sources"][0];
    assert_eq!(source["probe"]["runs"].as_array().unwrap().len(), 0);
    assert!(
        source["probe"]["failures"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("not found")
    );
    // Without an answer the start is not split, and the conclusion says so.
    assert_eq!(source["findings"][0]["conclusion"]["kind"], "look");
}

#[test]
fn findings_text_shows_conclusions_bars_and_what_was_read() {
    let output = run("epoch-baseline", &["--probe"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains('\x1b'));
    assert!(text.contains("READ") && text.contains("✓ probe"));
    assert!(text.contains("⇒ today's configuration is 33k of the median start 75k"));
    assert!(text.contains("████"));
    assert!(text.contains("loaded on demand, not sent at start"));
}
