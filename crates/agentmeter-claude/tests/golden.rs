//! Golden tests on small synthetic fixtures, one per format epoch. The fixtures hold no real
//! data: invented ids, an invented working directory, a one-letter message text.

use agentmeter_claude::read;
use agentmeter_core::model::Dataset;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn load(name: &str) -> Dataset {
    read(&[fixture(name)])
}

#[test]
fn streaming_duplicates_keep_the_largest_copy() {
    let d = load("epoch-streaming");
    // msg_A appears three times with output 5, 5, 300; msg_B once; the synthetic line is no request.
    assert_eq!(d.requests.len(), 2);
    assert_eq!(d.stats.duplicates_merged, 2);
    let a = d.requests.iter().find(|r| r.message_id == "msg_A").unwrap();
    assert_eq!((a.output, a.context()), (300, 3010));
    assert_eq!(d.requests.iter().map(|r| r.context()).sum::<u64>(), 6015);
    assert_eq!(
        d.stats.skipped_lines, 1,
        "the garbage line is counted, not fatal"
    );
    assert_eq!(d.stats.requests_flat_cache, 4);
    assert_eq!(d.stats.requests_with_ttl_split, 0);
    assert_eq!(a.project, "demo-app");
}

#[test]
fn cache_creation_only_nested_and_ttl_split() {
    let d = load("epoch-nested-cache");
    assert_eq!(d.requests.len(), 3);
    let sum = |f: fn(&agentmeter_core::Request) -> u64| d.requests.iter().map(f).sum::<u64>();
    // N1: flat total 0 but nested 1000 + 4000. N3: only the flat total, counted as 5 min.
    assert_eq!(sum(|r| r.cache_write_5m), 1000 + 300 + 50);
    assert_eq!(sum(|r| r.cache_write_1h), 4000);
    assert_eq!(sum(|r| r.context()), 5002 + 5301 + 5351);
    assert_eq!(d.stats.requests_with_ttl_split, 2);
    assert_eq!(d.stats.requests_flat_cache, 1);
}

#[test]
fn current_epoch_sidechain_replay_and_compaction() {
    let d = load("epoch-current");
    assert_eq!(
        d.requests.len(),
        4,
        "M1, M2, M3 and the sub-agent's own request"
    );
    assert_eq!(d.stats.replays_dropped, 1);
    assert_eq!(d.stats.duplicates_merged, 0);
    let sum = |f: fn(&agentmeter_core::Request) -> u64| d.requests.iter().map(f).sum::<u64>();
    assert_eq!(sum(|r| r.context()), 163_511);
    assert_eq!(sum(|r| r.cache_read), 20_000);
    assert_eq!(sum(|r| r.cache_write_1h), 20_500);
    assert_eq!(sum(|r| r.cache_write_5m), 123_000);
    assert_eq!(sum(|r| r.output), 260);
    assert_eq!(d.requests.iter().filter(|r| r.sub).count(), 1);
    assert_eq!(d.stats.first_version.as_deref(), Some("2.1.309"));

    assert_eq!(d.compactions.len(), 2);
    let (c1, c2) = (&d.compactions[0], &d.compactions[1]);
    assert_eq!(
        (c1.tokens_before, c1.tokens_after, c1.auto),
        (150_000, Some(20_000), Some(true))
    );
    assert_eq!((c2.tokens_before, c2.auto), (90_000, Some(false)));
    assert_eq!(c1.event_id, "c1");

    assert_eq!(
        d.session_starts.len(),
        1,
        "sub-agent files are no session starts"
    );
    assert!(d.session_starts[0].clean);
}

#[test]
fn unknown_type_and_new_fields_are_counted_and_change_nothing() {
    let d = load("epoch-current");
    assert_eq!(d.stats.unknown_types.get("future-event"), Some(&1));
    assert_eq!(d.stats.unknown_usage_fields.get("new_counter"), Some(&1));
    assert_eq!(d.stats.skipped_lines, 0);

    // The same logs without the unknown line and field give identical requests and compactions.
    let copy = std::env::temp_dir().join(format!("agentmeter-golden-{}", std::process::id()));
    let _ = fs::remove_dir_all(&copy);
    let source = fixture("epoch-current/projects/demo-app");
    for (relative, text) in [
        (
            "sess-main.jsonl",
            fs::read_to_string(source.join("sess-main.jsonl")).unwrap(),
        ),
        (
            "sess-main/subagents/agent-1.jsonl",
            fs::read_to_string(source.join("sess-main/subagents/agent-1.jsonl")).unwrap(),
        ),
    ] {
        let cleaned: String = text
            .lines()
            .filter(|line| !line.contains("future-event"))
            .map(|line| line.replace("\"new_counter\":7,", ""))
            .map(|line| line + "\n")
            .collect();
        let target = copy.join("projects/demo-app").join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, cleaned).unwrap();
    }
    let clean = read(std::slice::from_ref(&copy));
    fs::remove_dir_all(&copy).unwrap();
    assert_eq!(clean.stats.unknown_usage_fields.len(), 0);
    assert_eq!(clean.requests, d.requests);
    assert_eq!(clean.compactions, d.compactions);
}

#[test]
fn resumed_session_and_file_without_usage_are_marked() {
    let d = load("epoch-resumed");
    assert_eq!(d.stats.files, 2);
    assert_eq!(d.stats.files_without_usage, 1);
    let new = d
        .session_starts
        .iter()
        .find(|s| s.session == "sess-new")
        .unwrap();
    assert!(!new.clean);
    assert_eq!(new.reason, Some("inherited-history"));
    // The inherited request keeps its original session id.
    assert!(d.requests.iter().any(|r| r.session == "sess-old"));
}

#[test]
fn missing_directory_reads_as_nothing() {
    let d = read(&[fixture("does-not-exist")]);
    assert_eq!(d.stats.files, 0);
    assert!(d.requests.is_empty());
}

#[test]
fn project_is_the_directory_with_subagents_and_worktrees_folded_in() {
    let d = load("epoch-projects");
    assert_eq!(d.requests.len(), 3);
    // The sub-agent line has cwd "agent-abc123", the worktree line cwd "feat": neither is a project.
    assert!(
        d.requests.iter().all(|r| r.project == "demo-app"),
        "{:?}",
        d.requests
    );
}
