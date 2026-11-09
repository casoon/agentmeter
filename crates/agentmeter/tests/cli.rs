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
            "--tool-calls",
            "--transitions",
            "--subagents",
            "--instructions",
            fixtures().join("instr/claude/repo").to_str().unwrap(),
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    let documented = documented_fields();
    assert!(
        documented.len() == 35,
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

/// A directory with its own `CLAUDE.md`, an unreadable-looking import and a Codex file, under
/// the temp dir (so the ancestors are not the repository's).
fn instructions_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("agentmeter-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("CLAUDE.md"), "# x\n@gone.md\n").unwrap();
    std::fs::write(dir.join("sub/CLAUDE.md"), "nested\n").unwrap();
    std::fs::write(dir.join("AGENTS.md"), "codex\n").unwrap();
    std::fs::canonicalize(dir).unwrap()
}

#[test]
fn instructions_json_lists_files_with_state_and_findings_without_logs() {
    let dir = instructions_dir("json");
    let output = run(
        "does-not-exist-either",
        &[
            "--instructions",
            dir.to_str().unwrap(),
            "--tool",
            "claude",
            "--json",
        ],
    );
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let scan = &v["sources"][0]["instructions"];
    assert_eq!(scan["tool"], "claude");
    let files = scan["files"].as_array().unwrap();
    let state = |suffix: &str| {
        files
            .iter()
            .find(|f| f["path"].as_str().unwrap().ends_with(suffix))
            .map(|f| f["state"].as_str().unwrap().to_owned())
    };
    assert_eq!(state("/CLAUDE.md").as_deref(), Some("expected"));
    assert_eq!(state("sub/CLAUDE.md").as_deref(), Some("on_demand"));
    assert_eq!(state("gone.md").as_deref(), Some("missing"));
    assert_eq!(state("AGENTS.md").as_deref(), Some("not_loaded"));
    assert!(
        scan["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["rule"] == "broken-import")
    );
    // Only the expected file counts: "# x\n@gone.md\n" is 14 bytes.
    assert_eq!(scan["loaded_est_tokens"], 14u64.div_ceil(4));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn instructions_text_names_state_and_measure_and_fits_88_columns() {
    let dir = instructions_dir("text");
    let output = run(
        "does-not-exist-either",
        &["--instructions", dir.to_str().unwrap(), "--tool", "claude"],
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("INSTRUCTIONS")
            && text.contains("on demand")
            && text.contains("broken-import")
    );
    assert!(text.contains("tokens estimated as bytes / 4"));
    for line in text.lines() {
        assert!(line.chars().count() <= 88, "{line}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn instructions_codex_follows_its_own_rules_and_a_bad_directory_exits_two() {
    let dir = instructions_dir("codex");
    let output = run_both(
        "does-not-exist",
        "does-not-exist",
        &[
            "--instructions",
            dir.to_str().unwrap(),
            "--tool",
            "codex",
            "--json",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let scan = &v["sources"][0]["instructions"];
    assert_eq!(scan["tool"], "codex");
    let files = scan["files"].as_array().unwrap();
    // Codex reads AGENTS.md only; CLAUDE.md files are not its business.
    assert!(
        files
            .iter()
            .all(|f| f["path"].as_str().unwrap().ends_with("AGENTS.md"))
    );
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(
        run("epoch-current", &["--instructions", "/no/such/dir"])
            .status
            .code(),
        Some(2)
    );
}

/// A generated Claude log with repeats, a failing command, a huge result and secrets that must
/// never reach the report.
fn tool_call_config(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agentmeter-cli-tools-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("projects/-Users-me-GitHub-demo");
    std::fs::create_dir_all(&project).unwrap();
    let mut lines = Vec::new();
    let mut n = 0u32;
    let mut call = |name: &str, input: &str, text: &str, err: bool| {
        n += 1;
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-10-01T10:{n:02}:00.000Z","sessionId":"s1","message":{{"id":"m{n}","model":"claude-x","usage":{{"input_tokens":10,"output_tokens":5}},"content":[{{"type":"tool_use","id":"t{n}","name":"{name}","input":{input}}}]}}}}"#
        ));
        lines.push(format!(
            r#"{{"type":"user","timestamp":"2026-10-01T10:{n:02}:30.000Z","sessionId":"s1","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{n}","content":"{text}","is_error":{err}}}]}}}}"#
        ));
    };
    for _ in 0..7 {
        call("Read", r#"{"file_path":"/p/a.rs"}"#, "x", false);
    }
    for _ in 0..4 {
        call(
            "Bash",
            r#"{"command":"cargo test --secret-flag-xyz"}"#,
            "SECRET-OUTPUT-TEXT",
            true,
        );
    }
    call(
        "Bash",
        r#"{"command":"ls -R /"}"#,
        &"y".repeat(150_000),
        false,
    );
    std::fs::write(project.join("s1.jsonl"), lines.join("\n") + "\n").unwrap();
    dir
}

fn run_dir(config: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentmeter"))
        .args(args)
        .env("CLAUDE_CONFIG_DIR", config)
        .env("CODEX_HOME", fixtures().join("does-not-exist"))
        .env_remove("NO_COLOR")
        .output()
        .unwrap()
}

#[test]
fn tool_calls_json_has_three_rules_with_evidence_and_no_content() {
    let dir = tool_call_config("json");
    let output = run_dir(&dir, &["--tool-calls", "--json"]);
    assert!(output.status.success());
    let raw = String::from_utf8(output.stdout).unwrap();
    // Neither the command nor the output text may appear anywhere.
    for secret in [
        "secret-flag-xyz",
        "SECRET-OUTPUT-TEXT",
        "cargo test",
        "ls -R",
    ] {
        assert!(!raw.contains(secret), "{secret} leaked");
    }
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let tools = &v["sources"][0]["tool_calls"];
    assert_eq!(tools["calls"], 12);
    let rules = tools["rules"].as_array().unwrap();
    let ids: Vec<&str> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["repeat-read", "repeat-failure", "large-output"]);
    for rule in rules {
        assert_eq!(rule["status"], "observed");
        assert_eq!(rule["calibrated"], false);
        assert!(rule["from"].is_string() && rule["next_step"].is_string());
        assert!(!rule["counterexamples"].as_array().unwrap().is_empty());
        assert!(!rule["evidence"].as_array().unwrap().is_empty());
    }
    assert!(
        rules[1]["examples"][0]["label"]
            .as_str()
            .unwrap()
            .starts_with("Bash #")
    );
    // The default mode has no such key.
    let output = run_dir(&dir, &["--summary", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(v["sources"][0].get("tool_calls").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tool_calls_text_fits_and_codex_marks_rules_not_measurable() {
    let dir = tool_call_config("text");
    let output = run_dir(&dir, &["--tool-calls"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("TOOL CALLS") && text.contains("repeat-read") && text.contains("not counted")
    );
    assert!(!text.contains("SECRET-OUTPUT-TEXT") && !text.contains("secret-flag-xyz"));
    for line in text.lines() {
        assert!(line.chars().count() <= 88, "{line}");
    }
    let _ = std::fs::remove_dir_all(dir);
    let output = run_both(
        "does-not-exist",
        "codex-basic",
        &["--tool", "codex", "--tool-calls", "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rules = v["sources"][0]["tool_calls"]["rules"].as_array().unwrap();
    assert_eq!(rules[0]["status"], "not_measurable");
    assert_eq!(rules[1]["status"], "not_measurable");
}

/// Two Claude log files: a main session (start, reads, a compaction without a size after it, the
/// same reads again, a two-hour pause) and a resumed one that names the first as its parent.
fn transitions_config(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agentmeter-cli-trans-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("projects/-Users-me-GitHub-demo");
    std::fs::create_dir_all(&project).unwrap();
    let assistant = |session: &str, ts: &str, id: &str, tool: Option<(&str, &str)>| {
        let content = tool.map_or("[]".to_owned(), |(name, input)| {
            format!(r#"[{{"type":"tool_use","id":"tu-{id}","name":"{name}","input":{input}}}]"#)
        });
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{session}","parentUuid":null,"message":{{"id":"{id}","model":"claude-x","usage":{{"input_tokens":1000,"output_tokens":5}},"content":{content}}}}}"#
        )
    };
    let user = |session: &str, ts: &str, id: &str| {
        format!(
            r#"{{"type":"user","timestamp":"{ts}","sessionId":"{session}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"tu-{id}","content":"ok","is_error":false}}]}}}}"#
        )
    };
    let read = |p: &str| ("Read", format!(r#"{{"file_path":"{p}"}}"#));
    let mut main = Vec::new();
    for (i, p) in ["/p/a.rs", "/p/b.rs"].iter().enumerate() {
        let (tool, input) = read(p);
        let ts = format!("2026-10-01T10:0{i}:00.000Z");
        main.push(assistant(
            "s-main",
            &ts,
            &format!("m{i}"),
            Some((tool, &input)),
        ));
        main.push(user("s-main", &ts, &format!("m{i}")));
    }
    main.push(r#"{"type":"system","uuid":"c1","timestamp":"2026-10-01T10:10:00.000Z","sessionId":"s-main","compactMetadata":{"trigger":"auto","preTokens":900000}}"#.to_owned());
    for (i, p) in ["/p/a.rs", "/p/b.rs", "/p/new.rs"].iter().enumerate() {
        let (tool, input) = read(p);
        let ts = format!("2026-10-01T10:1{}:00.000Z", i + 1);
        main.push(assistant(
            "s-main",
            &ts,
            &format!("n{i}"),
            Some((tool, &input)),
        ));
        main.push(user("s-main", &ts, &format!("n{i}")));
    }
    main.push(assistant(
        "s-main",
        "2026-10-01T13:00:00.000Z",
        "late",
        None,
    ));
    std::fs::write(project.join("s-main.jsonl"), main.join("\n") + "\n").unwrap();
    // A resumed session: copied history under the parent's id, then its own request.
    let resumed = [
        assistant("s-main", "2026-10-01T10:00:00.000Z", "m0", None),
        assistant("s-resumed", "2026-10-02T09:00:00.000Z", "r1", None),
    ];
    std::fs::write(project.join("s-resumed.jsonl"), resumed.join("\n") + "\n").unwrap();
    dir
}

#[test]
fn transitions_json_separates_start_resume_compaction_pause_and_keeps_unknowns() {
    let dir = transitions_config("json");
    let output = run_dir(&dir, &["--transitions", "--tool-calls", "--json"]);
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let t = &v["sources"][0]["transitions"];
    let counts = &t["counts"];
    assert_eq!(counts["compactions"], 1);
    assert_eq!(counts["pauses"], 1);
    assert_eq!(counts["resumed_or_forked"], 1);
    let all = t["transitions"].as_array().unwrap();
    let of = |kind: &str| all.iter().find(|x| x["kind"] == kind).unwrap();
    // The parent is named by the log, not guessed from time.
    assert_eq!(of("resumed_or_forked")["parent"]["id"], "s-main");
    assert_eq!(of("resumed_or_forked")["parent"]["in_logs"], true);
    // A new chat in the same project is only a start, with no parent.
    assert!(of("start")["parent"].is_null());
    // The size after is unknown, not zero; the evidence is file and event.
    let c = of("compaction");
    assert!(c["context_after"].is_null());
    assert_eq!(c["context_before"], 900_000);
    assert_eq!(c["event"], "c1");
    assert!(c["file"].as_str().unwrap().ends_with("s-main.jsonl"));
    // Two of the three reads after it cover areas read before; /p/new.rs is new.
    assert_eq!(c["repeats"]["reads"], 3);
    assert_eq!(c["repeats"]["reread_areas"], 2);
    // The same compaction the summary counts, not an extra one.
    let summary = json_of(&dir, &["--summary", "--json"]);
    assert_eq!(
        summary["sources"][0]["summary"]["compactions"]["total"],
        counts["compactions"]
    );
    // The pause carries both contexts and no verdict field.
    assert!(of("pause")["repeats"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

fn json_of(config: &std::path::Path, args: &[&str]) -> serde_json::Value {
    serde_json::from_slice(&run_dir(config, args).stdout).unwrap()
}

#[test]
fn transitions_without_tool_data_stay_a_metadata_timeline() {
    let dir = transitions_config("meta");
    let v = json_of(&dir, &["--transitions", "--json"]);
    let t = &v["sources"][0]["transitions"];
    assert_eq!(t["tool_data"], false);
    assert!(
        t["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["repeats"].is_null())
    );
    assert!(v["sources"][0].get("tool_calls").is_none());
    let text = String::from_utf8(run_dir(&dir, &["--transitions", "--tool-calls"]).stdout).unwrap();
    assert!(text.contains("TRANSITIONS") && text.contains("cover areas read before"));
    for line in text.lines() {
        assert!(line.chars().count() <= 88, "{line}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn codex_transitions_leave_unknown_sizes_unknown() {
    let output = run_both(
        "does-not-exist",
        "codex-compacted",
        &["--tool", "codex", "--transitions", "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let all = v["sources"][0]["transitions"]["transitions"]
        .as_array()
        .unwrap();
    let c = all.iter().find(|x| x["kind"] == "compaction").unwrap();
    assert!(c["context_after"].is_null());
}

/// Claude logs for `--subagents`: a main session with two agents in its session folder (one
/// repeats a message of the parent: a replay), an agent log outside any session folder (no named
/// parent) and an independent chat of another project that runs at the same time.
fn subagents_config(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("agentmeter-cli-sub-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("projects/-Users-me-GitHub-demo");
    let other = dir.join("projects/-Users-me-GitHub-other");
    std::fs::create_dir_all(project.join("s-main/subagents")).unwrap();
    std::fs::create_dir_all(project.join("subagents")).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    // `agent` is the sessionId the line carries; sub-agent lines carry the parent's.
    let line = |session: &str,
                side: bool,
                ts: &str,
                id: &str,
                req: &str,
                tool: Option<(&str, &str)>| {
        let content = tool.map_or("[]".to_owned(), |(name, path)| {
            format!(
                r#"[{{"type":"tool_use","id":"tu-{id}","name":"{name}","input":{{"file_path":"{path}"}}}}]"#
            )
        });
        format!(
            r#"{{"type":"assistant","timestamp":"2026-10-01T{ts}.000Z","sessionId":"{session}","isSidechain":{side},"requestId":"{req}","parentUuid":null,"message":{{"id":"{id}","model":"claude-x","usage":{{"input_tokens":1000,"output_tokens":5}},"content":{content}}}}}"#
        )
    };
    let write = |path: std::path::PathBuf, lines: Vec<String>| {
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
    };
    write(
        project.join("s-main.jsonl"),
        vec![
            line(
                "s-main",
                false,
                "10:00:00",
                "m0",
                "",
                Some(("Read", "/p/shared.rs")),
            ),
            line("s-main", false, "10:06:00", "m1", "", None),
        ],
    );
    write(
        project.join("s-main/subagents/agent-a.jsonl"),
        vec![
            // Copy of the parent's first message under another request id.
            line("s-main", true, "10:00:00", "m0", "rq-copy", None),
            line(
                "s-main",
                true,
                "10:02:00",
                "a1",
                "",
                Some(("Read", "/p/only-a.rs")),
            ),
            line(
                "s-main",
                true,
                "10:04:00",
                "a2",
                "",
                Some(("Write", "/p/w.rs")),
            ),
            line(
                "s-main",
                true,
                "10:04:00",
                "a3",
                "",
                Some(("Read", "/p/shared.rs")),
            ),
        ],
    );
    write(
        project.join("s-main/subagents/agent-b.jsonl"),
        vec![
            line(
                "s-main",
                true,
                "10:03:00",
                "b1",
                "",
                Some(("Read", "/p/shared.rs")),
            ),
            line(
                "s-main",
                true,
                "10:03:00",
                "b2",
                "",
                Some(("Write", "/p/w.rs")),
            ),
            line(
                "s-main",
                true,
                "10:05:00",
                "b3",
                "",
                Some(("Read", "/p/only-b.rs")),
            ),
        ],
    );
    write(
        project.join("subagents/agent-o.jsonl"),
        vec![line("s-main", true, "10:02:30", "o1", "", None)],
    );
    write(
        other.join("s-other.jsonl"),
        vec![
            line("s-other", false, "10:03:30", "x1", "", None),
            line("s-other", false, "10:04:30", "x2", "", None),
        ],
    );
    dir
}

#[test]
fn subagents_split_direct_delegated_and_unassigned_without_double_counting() {
    let dir = subagents_config("split");
    let v = json_of(&dir, &["--subagents", "--summary", "--json"]);
    let s = &v["sources"][0];
    let t = &s["subagents"]["totals"];
    // The totals are the summary's: the copy in agent-a is a replay and counted once.
    assert_eq!(t["requests"], s["summary"]["requests"]);
    assert_eq!(t["input"], s["summary"]["input"]);
    let part = |key: &str, field: &str| t[key][field].as_u64().unwrap();
    assert_eq!(
        part("direct", "requests") + part("delegated", "requests") + part("unassigned", "requests"),
        t["requests"].as_u64().unwrap()
    );
    assert_eq!(
        part("delegated", "requests") + part("unassigned", "requests"),
        s["summary"]["sub_agent_requests"].as_u64().unwrap()
    );
    assert_eq!(
        part("direct", "requests"),
        4,
        "two of s-main, two of s-other"
    );
    assert_eq!(part("delegated", "requests"), 6, "three of each agent");
    assert_eq!(part("unassigned", "requests"), 1);
    // The agent log outside a session folder has no parent: listed apart, not linked by
    // project or time although it ran inside the session.
    let unassigned = s["subagents"]["unassigned"].as_array().unwrap();
    assert_eq!(unassigned.len(), 1);
    assert!(unassigned[0]["parent"].is_null() && unassigned[0]["depth"].is_null());
    let root = &s["subagents"]["roots"][0];
    assert_eq!(root["session"], "s-main");
    assert_eq!(root["agents"], 2);
    assert_eq!(root["children"][0]["parent"], "s-main");
    assert_eq!(root["children"][0]["parent_basis"], "session_folder");
    assert_eq!(
        root["inclusive_input"].as_u64().unwrap(),
        root["direct"]["input"].as_u64().unwrap() + root["delegated"]["input"].as_u64().unwrap()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn subagents_overlaps_tell_delegation_from_time_only_and_flag_shared_files() {
    let dir = subagents_config("overlap");
    let v = json_of(&dir, &["--subagents", "--tool-calls", "--json"]);
    let r = &v["sources"][0]["subagents"];
    assert_eq!(r["file_data"], true);
    let overlaps = r["overlaps"].as_array().unwrap();
    let pair = |a: &str, b: &str| {
        overlaps
            .iter()
            .find(|o| {
                (o["first"] == a && o["second"] == b) || (o["first"] == b && o["second"] == a)
            })
            .unwrap_or_else(|| panic!("no overlap of {a} and {b}"))
    };
    assert_eq!(pair("s-main", "agent-a")["relation"], "delegation");
    assert_eq!(pair("agent-a", "agent-b")["relation"], "same_task");
    // Another chat at the same time is only that: no relation in the logs.
    assert_eq!(pair("s-main", "s-other")["relation"], "time_only");
    assert_eq!(pair("s-other", "agent-b")["relation"], "time_only");
    // Both agents wrote /p/w.rs and read /p/shared.rs while both were running.
    let both = pair("agent-a", "agent-b");
    assert_eq!(both["shared_writes"], 1);
    assert_eq!(both["shared_reads"], 1);
    assert_eq!(both["examples"][0], "/p/w.rs");
    // Over the whole session: shared.rs was read by the session and both agents.
    let root = &r["roots"][0];
    assert_eq!(root["shared_reads"], 1);
    assert_eq!(root["shared_writes"], 1);
    assert_eq!(root["shared_reads_other_ranges"], 0);
    assert!(r["counts"]["overlaps_time_only"].as_u64().unwrap() >= 2);
    assert!(
        r["counterexamples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().contains("not a delegation"))
    );
    // Only paths, no tool input beyond them.
    assert!(!serde_json::to_string(r).unwrap().contains("tu-"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn subagents_without_tool_calls_name_the_missing_data_as_a_limit() {
    let dir = subagents_config("limit");
    let v = json_of(&dir, &["--subagents", "--json"]);
    let r = &v["sources"][0]["subagents"];
    assert_eq!(r["file_data"], false);
    assert!(r["roots"][0]["shared_writes"].is_null());
    assert!(r["overlaps"][0]["shared_writes"].is_null());
    assert!(
        r["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("--tool-calls"))
    );
    assert!(v["sources"][0].get("tool_calls").is_none());
    let text = String::from_utf8(run_dir(&dir, &["--subagents", "--tool-calls"]).stdout).unwrap();
    assert!(text.contains("SUBAGENTS") && text.contains("time only"));
    assert!(text.contains("The logs hold no run time"));
    for line in text.lines() {
        assert!(line.chars().count() <= 88, "{line}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Codex rollouts: a user thread, a child and a grandchild named by spawn records, an agent that
/// only carries a copy of the parent's history, an agent with no parent information and an
/// independent thread at the same time.
fn subagents_codex_home(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agentmeter-cli-subcx-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let day = dir.join("sessions/2026/10/01");
    std::fs::create_dir_all(&day).unwrap();
    let meta = |id: &str, source: &str, thread: &str| {
        format!(
            r#"{{"timestamp":"2026-10-01T10:00:00.000Z","type":"session_meta","payload":{{"id":"{id}","cli_version":"0.164.0","cwd":"/work/demo","thread_source":"{thread}","source":{source}}}}}"#
        )
    };
    let tokens = |ts: &str, input: u64| {
        format!(
            r#"{{"timestamp":"2026-10-01T{ts}.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":10,"total_tokens":{t}}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":10,"total_tokens":{t}}}}}}}}}"#,
            t = input + 10
        )
    };
    let spawn = |parent: &str| {
        format!(r#"{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent}","depth":1}}}}}}"#)
    };
    let write = |id: &str, lines: Vec<String>| {
        std::fs::write(
            day.join(format!("rollout-2026-10-01T10-00-00-{id}.jsonl")),
            lines.join("\n") + "\n",
        )
        .unwrap();
    };
    write(
        "cx-r",
        vec![
            meta("cx-r", r#""cli""#, "user"),
            tokens("10:00:10", 1000),
            tokens("10:10:10", 2000),
        ],
    );
    write(
        "cx-c",
        vec![
            meta("cx-c", &spawn("cx-r"), "subagent"),
            tokens("10:01:10", 1100),
            tokens("10:02:10", 1200),
        ],
    );
    write(
        "cx-g",
        vec![
            meta("cx-g", &spawn("cx-c"), "subagent"),
            tokens("10:01:30", 1300),
            tokens("10:02:30", 1400),
        ],
    );
    write(
        "cx-k",
        vec![
            meta("cx-k", r#""cli""#, "subagent"),
            meta("cx-r", r#""cli""#, "user"),
            tokens("10:06:00", 1500),
        ],
    );
    write(
        "cx-h",
        vec![
            meta("cx-h", r#""cli""#, "subagent"),
            tokens("10:05:00", 1600),
        ],
    );
    write(
        "cx-i",
        vec![meta("cx-i", r#""cli""#, "user"), tokens("10:03:00", 1700)],
    );
    dir
}

#[test]
fn codex_subagents_follow_spawn_records_through_nested_delegation_and_keep_unknowns() {
    let dir = subagents_codex_home("nested");
    let output = Command::new(env!("CARGO_BIN_EXE_agentmeter"))
        .args([
            "--tool",
            "codex",
            "--subagents",
            "--summary",
            "--tool-calls",
            "--json",
        ])
        .env("CLAUDE_CONFIG_DIR", fixtures().join("does-not-exist"))
        .env("CODEX_HOME", &dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let s = &v["sources"][0];
    let r = &s["subagents"];
    assert_eq!(r["totals"]["requests"], s["summary"]["requests"]);
    assert_eq!(r["counts"]["agents"], 4);
    assert_eq!(r["counts"]["nested_agents"], 1);
    assert_eq!(r["counts"]["max_depth"], 2);
    assert_eq!(r["counts"]["unassigned_agents"], 1);
    let root = r["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["session"] == "cx-r")
        .unwrap();
    assert_eq!(root["agents"], 3);
    let child = |id: &str| {
        root["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["agent"] == id)
            .unwrap()
    };
    assert_eq!(
        (
            child("cx-c")["depth"].as_u64(),
            child("cx-c")["parent_basis"].as_str()
        ),
        (Some(1), Some("spawn_record"))
    );
    assert_eq!(
        (
            child("cx-g")["depth"].as_u64(),
            child("cx-g")["parent"].as_str()
        ),
        (Some(2), Some("cx-c"))
    );
    // The parent of cx-k is only the history copied into its rollout: named, but weaker.
    assert_eq!(child("cx-k")["parent_basis"], "copied_history");
    // cx-h names no parent and stays unassigned; the independent thread has no agents.
    assert_eq!(r["unassigned"][0]["agent"], "cx-h");
    assert!(r["unassigned"][0]["parent"].is_null());
    assert!(
        r["roots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["session"] != "cx-i")
    );
    // Codex logs no file targets: shared files are a limit, not zero.
    assert_eq!(r["file_data"], false);
    assert!(root["shared_writes"].is_null());
    assert!(
        r["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("no file targets"))
    );
    let relation = |a: &str, b: &str| {
        r["overlaps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| {
                (o["first"] == a && o["second"] == b) || (o["first"] == b && o["second"] == a)
            })
            .map(|o| o["relation"].clone())
    };
    assert_eq!(
        relation("cx-r", "cx-g"),
        Some(serde_json::json!("delegation"))
    );
    assert_eq!(
        relation("cx-c", "cx-g"),
        Some(serde_json::json!("delegation"))
    );
    assert_eq!(
        relation("cx-i", "cx-r"),
        Some(serde_json::json!("time_only"))
    );
    let _ = std::fs::remove_dir_all(dir);
}
