//! Tool calls read from a generated Codex rollout (`--tool-calls`).

use agentmeter_codex::read_with_tools;
use agentmeter_core::toolcalls::evaluate;
use std::fs;

#[test]
fn only_sizes_are_measurable_because_arguments_are_scripts() {
    let dir = std::env::temp_dir().join(format!("agentmeter-cxtools-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let day = dir.join("sessions/2026/10/01");
    fs::create_dir_all(&day).unwrap();
    let big = "z".repeat(120_000);
    let lines = [
        r#"{"type":"session_meta","timestamp":"2026-10-01T10:00:00.000Z","payload":{"id":"cx","cwd":"/p/demo"}}"#.to_owned(),
        r#"{"type":"response_item","timestamp":"2026-10-01T10:00:01.000Z","payload":{"type":"custom_tool_call","name":"exec","call_id":"c1","input":"SECRET-SCRIPT"}}"#.to_owned(),
        format!(r#"{{"type":"response_item","timestamp":"2026-10-01T10:00:02.000Z","payload":{{"type":"custom_tool_call_output","call_id":"c1","output":[{{"type":"input_text","text":"{big}"}}]}}}}"#),
        r#"{"type":"response_item","timestamp":"2026-10-01T10:00:03.000Z","payload":{"type":"function_call","name":"sleep","call_id":"c2","arguments":"{}"}}"#.to_owned(),
        r#"{"type":"response_item","timestamp":"2026-10-01T10:00:03.500Z","payload":{"type":"function_call","name":"sleep","call_id":"c2","arguments":"{}"}}"#.to_owned(),
    ];
    fs::write(
        day.join("rollout-2026-10-01T10-00-00-cx.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    let (dataset, data) = read_with_tools(std::slice::from_ref(&dir), true);
    assert_eq!(data.stats.calls, 2);
    assert_eq!(data.stats.duplicates_merged, 1);
    assert_eq!(data.stats.calls_without_result, 1);
    let a = evaluate(&data, &dataset.files, &dataset.compactions, None);
    assert_eq!(a.rules[0].status, "not_measurable");
    assert_eq!(a.rules[1].status, "not_measurable");
    assert_eq!((a.rules[2].status, a.rules[2].count), ("observed", Some(1)));
    assert_eq!(a.rules[2].examples[0].label, "exec");
    let (_, off) = read_with_tools(std::slice::from_ref(&dir), false);
    assert!(off.events.is_empty());
    let _ = fs::remove_dir_all(dir);
}
