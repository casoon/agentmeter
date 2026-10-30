//! Golden tests on small synthetic Codex rollouts, one fixture per format situation. Invented
//! ids and working directories only.

use agentmeter_codex::read;
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
fn deltas_of_the_running_total_repeats_ignored() {
    let d = load("codex-basic");
    // Two real changes; the third token_count repeats the total, the last has no info.
    assert_eq!(d.requests.len(), 2);
    assert_eq!(d.stats.duplicates_merged, 1);
    let (a, b) = (&d.requests[0], &d.requests[1]);
    assert_eq!(
        (a.context(), a.fresh, a.cache_read, a.output),
        (1000, 1000, 0, 50)
    );
    assert_eq!(
        (b.context(), b.fresh, b.cache_read, b.output),
        (2500, 1500, 1000, 100)
    );
    assert_eq!(a.window, Some(258_400));
    assert_eq!(a.model, "m-code");
    assert_eq!(a.project, "demo-code");
    assert_eq!(a.message_id, "ordinal:4");
    assert_eq!(d.requests.iter().map(|r| r.context()).sum::<u64>(), 3500);
    assert_eq!(d.stats.usage_delta_mismatches, 0);
    assert_eq!(d.stats.first_version.as_deref(), Some("0.125.0"));
    assert!(d.session_starts[0].clean);
}

#[test]
fn cache_writes_are_not_measured_unless_the_log_records_them() {
    assert!(!load("codex-basic").stats.cache_write_measured);
    let d = load("codex-cache-write");
    assert!(d.stats.cache_write_measured);
    let (a, b) = (&d.requests[0], &d.requests[1]);
    assert_eq!((a.context(), a.cache_write(), a.fresh), (2000, 1500, 500));
    assert_eq!(
        (b.context(), b.cache_write(), b.cache_read, b.fresh),
        (2500, 0, 1500, 1000)
    );
}

#[test]
fn compaction_is_recognised_once_with_unknown_trigger_and_size() {
    let d = load("codex-compacted");
    assert_eq!(d.requests.len(), 2);
    assert_eq!(
        d.compactions.len(),
        1,
        "the copy in archived_sessions repeats the same response id"
    );
    let c = &d.compactions[0];
    assert_eq!(
        (c.tokens_before, c.tokens_after, c.auto),
        (300_000, None, None)
    );
    assert_eq!(c.event_id, "cr-1");
    assert_eq!(
        d.stats.unknown_types.len(),
        0,
        "token_usage_record is a known type"
    );
}

#[test]
fn file_without_token_count_and_damaged_compressed_file_are_data_gaps() {
    let d = load("codex-no-token-count");
    assert_eq!(d.stats.files, 1);
    assert_eq!(d.stats.files_without_usage, 1);
    // The .zst fixture holds text that is no zstd frame.
    assert_eq!(d.stats.compressed_files_unreadable, 1);
    assert_eq!(d.stats.compressed_files_read, 0);
    assert!(d.requests.is_empty());
}

#[test]
fn subagent_replay_of_the_parent_is_not_counted_twice() {
    let d = load("codex-subagent");
    assert_eq!(d.requests.len(), 2);
    assert_eq!(d.stats.replays_dropped, 1);
    assert_eq!(d.requests.iter().filter(|r| r.sub).count(), 1);
    let sub = d.requests.iter().find(|r| r.sub).unwrap();
    assert_eq!((sub.context(), sub.cache_read, sub.output), (400, 100, 20));
    assert_eq!(
        d.session_starts.len(),
        1,
        "a sub-agent rollout is no session start"
    );
}

#[test]
fn unknown_type_and_new_field_are_counted_and_change_nothing() {
    let d = load("codex-basic");
    assert_eq!(d.stats.unknown_types.get("future-event"), Some(&1));
    assert_eq!(d.stats.unknown_usage_fields.get("new_counter"), Some(&1));

    let copy = std::env::temp_dir().join(format!("agentmeter-codex-golden-{}", std::process::id()));
    let _ = fs::remove_dir_all(&copy);
    let source = fixture("codex-basic/sessions/2026/05/01");
    let name = "rollout-2026-05-01T10-00-00-cx-1.jsonl";
    let cleaned: String = fs::read_to_string(source.join(name))
        .unwrap()
        .lines()
        .filter(|line| !line.contains("future-event"))
        .map(|line| line.replace(",\"new_counter\":7", "") + "\n")
        .collect();
    let target = copy.join("sessions/2026/05/01").join(name);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, cleaned).unwrap();
    let clean = read(std::slice::from_ref(&copy));
    fs::remove_dir_all(&copy).unwrap();
    assert_eq!(clean.stats.unknown_usage_fields.len(), 0);
    assert_eq!(clean.requests, d.requests);
    assert_eq!(clean.compactions, d.compactions);
}

#[test]
fn missing_directory_reads_as_nothing() {
    let d = read(&[fixture("does-not-exist")]);
    assert_eq!(d.stats.files, 0);
    assert!(d.requests.is_empty());
}

/// Writes `text` plain and/or compressed with ruzstd into a fresh Codex home and reads it.
fn read_copy(tag: &str, plain: Option<&[u8]>, compressed: Option<Vec<u8>>) -> Dataset {
    let home = std::env::temp_dir().join(format!("agentmeter-zst-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    let dir = home.join("sessions/2026/05/01");
    fs::create_dir_all(&dir).unwrap();
    let name = "rollout-2026-05-01T10-00-00-cx-1.jsonl";
    if let Some(plain) = plain {
        fs::write(dir.join(name), plain).unwrap();
    }
    if let Some(bytes) = compressed {
        fs::write(dir.join(format!("{name}.zst")), bytes).unwrap();
    }
    let dataset = read(std::slice::from_ref(&home));
    fs::remove_dir_all(&home).unwrap();
    dataset
}

fn basic_text() -> Vec<u8> {
    fs::read(fixture(
        "codex-basic/sessions/2026/05/01/rollout-2026-05-01T10-00-00-cx-1.jsonl",
    ))
    .unwrap()
}

fn zst(bytes: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(bytes, ruzstd::encoding::CompressionLevel::Fastest)
}

#[test]
fn compressed_rollout_gives_the_same_numbers_as_the_plain_one() {
    let text = basic_text();
    let plain = read_copy("plain", Some(&text), None);
    let packed = read_copy("packed", None, Some(zst(&text)));
    assert_eq!(packed.requests, plain.requests);
    assert_eq!(packed.requests.len(), 2);
    assert_eq!(packed.stats.compressed_files_read, 1);
    assert_eq!(packed.stats.compressed_files_unreadable, 0);
    assert_eq!(packed.stats.lines, plain.stats.lines);
    assert_eq!(packed.session_starts.len(), 1);
    assert_eq!(packed.session_starts[0].session, "cx-1");
}

#[test]
fn two_frames_are_read_like_zstd_dc() {
    let text = basic_text();
    // Split after the first line so each frame holds whole lines.
    let cut = text.iter().position(|b| *b == b'\n').unwrap() + 1;
    let (first, second) = text.split_at(cut);
    let mut both = zst(first);
    both.extend(zst(second));
    let packed = read_copy("frames", None, Some(both));
    assert_eq!(packed.requests.len(), 2);
    assert_eq!(packed.stats.compressed_files_unreadable, 0);
}

#[test]
fn damaged_compressed_file_is_counted_and_keeps_what_was_read() {
    let text = basic_text();
    let mut bytes = zst(&text);
    bytes.truncate(bytes.len() - 6);
    let damaged = read_copy("damaged", None, Some(bytes));
    assert_eq!(damaged.stats.compressed_files_unreadable, 1);
    let garbage = read_copy("garbage", None, Some(b"nonsense".to_vec()));
    assert_eq!(garbage.stats.compressed_files_unreadable, 1);
    assert_eq!(garbage.stats.files, 0);
}

#[test]
fn plain_text_wins_over_a_compressed_copy() {
    let text = basic_text();
    let d = read_copy("both", Some(&text), Some(zst(&text)));
    assert_eq!(d.requests.len(), 2);
    assert_eq!(d.stats.files, 1);
    assert_eq!(d.stats.compressed_files_shadowed, 1);
    assert_eq!(d.stats.compressed_files_read, 0);
}

#[test]
fn forked_rollout_does_not_inherit_the_parents_running_total() {
    let d = load("codex-fork");
    // The first event carries the parent's total (8 M) but its own request is empty.
    assert_eq!(d.requests.len(), 1);
    assert_eq!((d.requests[0].context(), d.requests[0].output), (300, 40));
    assert_eq!(d.stats.usage_delta_mismatches, 1);
}
