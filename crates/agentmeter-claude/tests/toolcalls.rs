//! Tool calls read from a generated Claude log (`--tool-calls`).

use agentmeter_claude::read_with_tools;
use agentmeter_core::toolcalls::{ToolKind, ToolStatus, evaluate};
use std::fs;
use std::path::PathBuf;

pub fn assistant(minute: u32, msg: &str, id: &str, name: &str, input: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"2026-10-01T10:{minute:02}:00.000Z","sessionId":"s1","message":{{"id":"{msg}","model":"claude-x","usage":{{"input_tokens":10,"output_tokens":5}},"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
    )
}

pub fn result(minute: u32, id: &str, text: &str, is_error: bool) -> String {
    format!(
        r#"{{"type":"user","timestamp":"2026-10-01T10:{minute:02}:30.000Z","sessionId":"s1","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"{text}","is_error":{is_error}}}]}}}}"#
    )
}

/// A log with every case the rules know: see the assertions.
pub fn scenario() -> Vec<String> {
    let mut l = Vec::new();
    let mut n = 0u32;
    let mut call = |l: &mut Vec<String>, name: &str, input: &str, text: &str, err: bool| {
        n += 1;
        let id = format!("t{n}");
        l.push(assistant(n % 60, &format!("m{n}"), &id, name, input));
        l.push(result(n % 60, &id, text, err));
    };
    // Seven unchanged reads of one file: six repeats.
    for _ in 0..7 {
        call(&mut l, "Read", r#"{"file_path":"/p/a.rs"}"#, "x", false);
    }
    // A read, an edit, a read: justified. Two separate ranges: separate areas.
    call(&mut l, "Read", r#"{"file_path":"/p/b.rs"}"#, "x", false);
    call(
        &mut l,
        "Edit",
        r#"{"file_path":"/p/b.rs","old_string":"SECRET-EDIT"}"#,
        "ok",
        false,
    );
    call(&mut l, "Read", r#"{"file_path":"/p/b.rs"}"#, "x", false);
    call(
        &mut l,
        "Read",
        r#"{"file_path":"/p/b.rs","offset":0,"limit":10}"#,
        "x",
        false,
    );
    call(
        &mut l,
        "Read",
        r#"{"file_path":"/p/b.rs","offset":100,"limit":10}"#,
        "x",
        false,
    );
    // The same failing command four times.
    for _ in 0..4 {
        call(
            &mut l,
            "Bash",
            r#"{"command":"cargo test --secret-flag-xyz","description":"d"}"#,
            "SECRET-OUTPUT-TEXT",
            true,
        );
    }
    // A huge result.
    let huge = "y".repeat(150_000);
    call(&mut l, "Bash", r#"{"command":"ls -R /"}"#, &huge, false);
    // A streaming copy of the last call's tool_use (same id) and a call without a result.
    l.push(assistant(
        50,
        "m-dup",
        "t17",
        "Bash",
        r#"{"command":"ls -R /"}"#,
    ));
    l.push(assistant(
        51,
        "m-open",
        "t-open",
        "Read",
        r#"{"file_path":"/p/c.rs"}"#,
    ));
    l
}

pub fn write_config(name: &str, lines: &[String]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentmeter-tools-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let project = dir.join("projects/-Users-me-GitHub-demo");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("s1.jsonl"), lines.join("\n") + "\n").unwrap();
    dir
}

#[test]
fn calls_and_results_are_joined_and_duplicates_merged() {
    let dir = write_config("join", &scenario());
    let (dataset, data) = read_with_tools(std::slice::from_ref(&dir), true);
    assert_eq!(data.stats.duplicates_merged, 1);
    assert_eq!(data.stats.calls, 18);
    assert_eq!(data.stats.calls_without_result, 1);
    let open = data.events.iter().find(|e| e.call_id == "t-open").unwrap();
    assert_eq!(
        (open.status, open.output_bytes, open.has_result),
        (ToolStatus::Unknown, None, false)
    );
    let edit = data
        .events
        .iter()
        .find(|e| e.kind == ToolKind::Write)
        .unwrap();
    assert_eq!(edit.target.as_deref(), Some("/p/b.rs"));
    let huge = data
        .events
        .iter()
        .find(|e| e.output_bytes == Some(150_000))
        .unwrap();
    assert_eq!(huge.tool, "Bash");
    // Commands are fingerprints; no text is kept in the event.
    let failing = data
        .events
        .iter()
        .find(|e| e.status == ToolStatus::Error)
        .unwrap();
    assert!(failing.fingerprint.is_some() && failing.target.is_none());
    assert!(!dataset.requests.is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_rules_see_repeats_but_not_justified_rereads_or_other_ranges() {
    let dir = write_config("rules", &scenario());
    let (dataset, data) = read_with_tools(std::slice::from_ref(&dir), true);
    let a = evaluate(&data, &dataset.files, &dataset.compactions, None);
    let [read, failure, large] = &a.rules[..] else {
        panic!("three rules");
    };
    assert_eq!((read.status, read.count), ("observed", Some(6)));
    assert!(read.counterexamples[0].starts_with("1 re-reads after an observed edit"));
    assert_eq!(read.examples[0].label, "/p/a.rs");
    assert_eq!((failure.status, failure.count), ("observed", Some(3)));
    assert!(failure.examples[0].label.starts_with("Bash #"));
    assert_eq!((large.status, large.count), ("observed", Some(1)));
    // Evidence points at the file and call id, never at text.
    assert!(read.evidence[0].file.ends_with("s1.jsonl") && read.evidence[0].event.starts_with('t'));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn without_the_option_no_tool_data_is_read() {
    let dir = write_config("off", &scenario());
    let (_, data) = read_with_tools(std::slice::from_ref(&dir), false);
    assert!(data.events.is_empty());
    let _ = fs::remove_dir_all(dir);
}
