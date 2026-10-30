//! Optional checks against real rollouts; each passes without doing anything unless its
//! environment variable is set.
//!
//! `AGENTMETER_REAL_LOGS=1` counts the maintainer's own rollouts a second time, independently of
//! the adapter, straight from the JSON with the rules written down in `plan/status.md`
//! (running total changed, first occurrence of an event wins) and compares the sums exactly.
//!
//! `AGENTMETER_CODEX_REF_DIR=<dir>` reads the public benchmark traces (CC-BY-4.0, never part of
//! this repository) for a local look at an older format epoch and only prints the totals.

use agentmeter_codex::{config_dirs, read};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

fn rollouts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            rollouts(&path, out);
        } else if path.extension().is_some_and(|e| e == "jsonl" || e == "zst") {
            out.push(path);
        }
    }
}

/// (requests, input, cached, output) counted straight from the files.
fn independent_count(homes: &[PathBuf]) -> (u64, u64, u64, u64) {
    let mut seen = HashSet::new();
    let (mut requests, mut input, mut cached, mut output) = (0, 0, 0, 0);
    for home in homes {
        let mut files = Vec::new();
        rollouts(&home.join("sessions"), &mut files);
        rollouts(&home.join("archived_sessions"), &mut files);
        for file in files {
            let mut previous = (0u64, 0u64, 0u64);
            let mut first = true;
            let Ok(opened) = fs::File::open(&file) else {
                continue;
            };
            let stream: Box<dyn Read> = if file.extension().is_some_and(|e| e == "zst") {
                // Plain text wins over a compressed copy of the same rollout.
                if file.with_extension("").exists() {
                    continue;
                }
                match ruzstd::decoding::StreamingDecoder::new(BufReader::new(opened)) {
                    Ok(decoder) => Box::new(decoder),
                    Err(_) => continue,
                }
            } else {
                Box::new(opened)
            };
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let payload = &value["payload"];
                if value["type"] != "event_msg" || payload["type"] != "token_count" {
                    continue;
                }
                let total = &payload["info"]["total_token_usage"];
                let get = |name: &str| total[name].as_u64();
                let (Some(i), Some(c), Some(o)) = (
                    get("input_tokens"),
                    get("cached_input_tokens"),
                    get("output_tokens"),
                ) else {
                    continue;
                };
                if (i, c, o) == previous {
                    continue;
                }
                if !seen.insert((value["timestamp"].to_string(), i, c, o)) {
                    continue;
                }
                // A counter that went backwards starts over: the event's own value stands.
                let own = |last: &Value| {
                    let get = |name: &str| last[name].as_u64().unwrap_or(0);
                    (
                        get("input_tokens"),
                        get("cached_input_tokens"),
                        get("output_tokens"),
                    )
                };
                let last = &payload["info"]["last_token_usage"];
                let was_first = std::mem::replace(&mut first, false);
                // A forked rollout starts with its parent's running total: only the event's own value counts.
                let (di, dc, d_o) = if was_first && (own(last).0, own(last).2) != (i, o) {
                    own(last)
                } else if i >= previous.0 && c >= previous.1 && o >= previous.2 {
                    (i - previous.0, c - previous.1, o - previous.2)
                } else {
                    let last = &payload["info"]["last_token_usage"];
                    let get = |name: &str| last[name].as_u64().unwrap_or(0);
                    (
                        get("input_tokens"),
                        get("cached_input_tokens"),
                        get("output_tokens"),
                    )
                };
                previous = (i, c, o);
                if di == 0 && d_o == 0 {
                    continue;
                }
                requests += 1;
                input += di;
                cached += dc;
                output += d_o;
            }
        }
    }
    (requests, input, cached, output)
}

#[test]
fn own_rollouts_match_an_independent_count() {
    if std::env::var_os("AGENTMETER_REAL_LOGS").is_none_or(|v| v != "1") {
        eprintln!("skipped: set AGENTMETER_REAL_LOGS=1 to run against real rollouts");
        return;
    }
    let homes = config_dirs();
    let dataset = read(&homes);
    if dataset.stats.files == 0 {
        eprintln!("no readable Codex rollouts; nothing to compare");
        return;
    }
    let sums = (
        dataset.requests.len() as u64,
        dataset.requests.iter().map(|r| r.context()).sum::<u64>(),
        dataset.requests.iter().map(|r| r.cache_read).sum::<u64>(),
        dataset.requests.iter().map(|r| r.output).sum::<u64>(),
    );
    println!(
        "adapter: {} requests, input {}, cached {}, output {}; {} files read, {} compressed read, {} unreadable",
        sums.0,
        sums.1,
        sums.2,
        sums.3,
        dataset.stats.files,
        dataset.stats.compressed_files_read,
        dataset.stats.compressed_files_unreadable
    );
    let independent = independent_count(&homes);
    println!("independent: {independent:?}");
    assert_eq!(sums, independent);
}

#[test]
fn public_traces_print_their_totals() {
    let Some(dir) = std::env::var_os("AGENTMETER_CODEX_REF_DIR").map(PathBuf::from) else {
        eprintln!("skipped: set AGENTMETER_CODEX_REF_DIR to read the public traces");
        return;
    };
    // The traces keep one Codex home per run in a `session_logs` directory.
    fn homes(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "session_logs")
                    && path.join("sessions").is_dir()
                {
                    out.push(path);
                } else {
                    homes(&path, out);
                }
            }
        }
    }
    let mut found = Vec::new();
    homes(&dir, &mut found);
    found.sort();
    let dataset = read(&found);
    println!(
        "public traces: {} files, {} requests, input {}, cache writes measured: {}, delta mismatches: {}, versions {:?} - {:?}",
        dataset.stats.files,
        dataset.requests.len(),
        dataset.requests.iter().map(|r| r.context()).sum::<u64>(),
        dataset.stats.cache_write_measured,
        dataset.stats.usage_delta_mismatches,
        dataset.stats.first_version,
        dataset.stats.last_version
    );
}
