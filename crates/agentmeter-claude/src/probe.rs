//! `--probe`: asks Claude Code itself what a session start holds.
//!
//! Runs `claude -p /context --no-session-persistence --output-format json` in a directory. The
//! command is answered locally (no model request, no cost, no session file) and returns a
//! Markdown report; only its tables are read: categories, memory files and MCP tools with token
//! estimates. Starting `claude` loads the configuration of that directory, including its MCP
//! servers and hooks, which is why the probe only runs when asked for.

use agentmeter_core::model::{ContextProbe, Probe, ProbeFailure, ProbeItem};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one probe may take; `claude` connects MCP servers before it answers.
const PROBE_TIMEOUT: Duration = Duration::from_secs(90);

/// The program to run: `AGENTMETER_CLAUDE` (for tests and unusual installs), else `claude`.
fn program() -> String {
    std::env::var("AGENTMETER_CLAUDE")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "claude".to_owned())
}

const ARGS: &[&str] = &[
    "-p",
    "/context",
    "--no-session-persistence",
    "--output-format",
    "json",
];

/// Runs one probe per place, all at once. `places` pairs a name with a directory.
pub fn run(places: &[(String, PathBuf)]) -> Probe {
    let program = program();
    let handles: Vec<_> = places
        .iter()
        .cloned()
        .map(|(place, dir)| {
            let program = program.clone();
            std::thread::spawn(move || (place.clone(), probe_one(&program, &place, &dir)))
        })
        .collect();
    let mut probe = Probe {
        command: format!("{program} {}", ARGS.join(" ")),
        ..Probe::default()
    };
    for handle in handles {
        match handle.join() {
            Ok((_, Ok(run))) => probe.runs.push(run),
            Ok((place, Err(reason))) => probe.failures.push(ProbeFailure { place, reason }),
            Err(_) => probe.failures.push(ProbeFailure {
                place: "(unknown)".to_owned(),
                reason: "probe thread panicked".to_owned(),
            }),
        }
    }
    probe
}

fn probe_one(program: &str, place: &str, dir: &Path) -> Result<ContextProbe, String> {
    let mut child = Command::new(program)
        .args(ARGS)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("`{program}` not found on PATH"),
            _ => format!("could not start `{program}`: {e}"),
        })?;
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).map(|_| out)
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} s", PROBE_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("waiting for `{program}` failed: {e}")),
        }
    };
    let output = reader
        .join()
        .map_err(|_| "reading the answer failed".to_owned())?
        .map_err(|e| format!("reading the answer failed: {e}"))?;
    if !status.success() {
        return Err(format!("`{program}` exited with {status}"));
    }
    parse(&output, place, &dir.display().to_string())
}

/// Reads the JSON answer of `claude -p /context --output-format json`.
pub fn parse(json: &str, place: &str, dir: &str) -> Result<ContextProbe, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "answer is not JSON".to_owned())?;
    if value["is_error"].as_bool() == Some(true) {
        return Err("claude reported an error".to_owned());
    }
    let report = value["result"]
        .as_str()
        .ok_or_else(|| "answer has no result text".to_owned())?;
    let mut probe = ContextProbe {
        place: place.to_owned(),
        dir: dir.to_owned(),
        model: None,
        total: 0,
        sent: Vec::new(),
        deferred: Vec::new(),
        memory_files: Vec::new(),
        mcp_servers: Vec::new(),
    };
    let mut section = "";
    let mut saw_categories = false;
    for line in report.lines().map(str::trim) {
        if let Some(model) = line.strip_prefix("**Model:**") {
            probe.model = Some(model.trim().to_owned());
        } else if let Some(heading) = line.strip_prefix("### ") {
            section = match heading.trim() {
                "Estimated usage by category" => "categories",
                "Memory Files" => "memory",
                "MCP Tools" => "mcp",
                _ => "",
            };
        } else if let Some(cells) = table_row(line) {
            match (section, cells.as_slice()) {
                ("categories", [name, tokens, ..]) => {
                    let Some(tokens) = parse_tokens(tokens) else {
                        continue;
                    };
                    saw_categories = true;
                    let item = ProbeItem {
                        label: name.to_string(),
                        tokens,
                    };
                    if matches!(*name, "Free space" | "Autocompact buffer") {
                        continue;
                    } else if name.contains("(deferred)") {
                        probe.deferred.push(item);
                    } else {
                        probe.sent.push(item);
                    }
                }
                ("memory", [_, path, tokens, ..]) => {
                    if let Some(tokens) = parse_tokens(tokens) {
                        probe.memory_files.push(ProbeItem {
                            label: path.to_string(),
                            tokens,
                        });
                    }
                }
                ("mcp", [_, server, tokens, ..]) => {
                    if let Some(tokens) = parse_tokens(tokens) {
                        match probe.mcp_servers.iter_mut().find(|s| s.label == *server) {
                            Some(s) => s.tokens += tokens,
                            None => probe.mcp_servers.push(ProbeItem {
                                label: server.to_string(),
                                tokens,
                            }),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if !saw_categories {
        return Err(
            "answer has no category table; the /context format may have changed".to_owned(),
        );
    }
    for list in [
        &mut probe.sent,
        &mut probe.deferred,
        &mut probe.memory_files,
        &mut probe.mcp_servers,
    ] {
        list.sort_by_key(|i| std::cmp::Reverse(i.tokens));
    }
    probe.total = probe.sent.iter().map(|i| i.tokens).sum();
    Ok(probe)
}

/// The cells of a Markdown table row; `None` for other lines and the `|---|` separator.
fn table_row(line: &str) -> Option<Vec<&str>> {
    let inner = line.strip_prefix('|')?.strip_suffix('|')?;
    let cells: Vec<&str> = inner.split('|').map(str::trim).collect();
    if cells
        .iter()
        .all(|c| c.chars().all(|ch| ch == '-' || ch == ':'))
    {
        return None;
    }
    Some(cells)
}

/// `3k`, `19.1k`, `1.2m`, `514`, `~70`, `< 20` as tokens; `None` for a header cell.
fn parse_tokens(cell: &str) -> Option<u64> {
    let cell = cell.trim_start_matches(['~', '<', ' ']).trim();
    let (number, factor) = match cell.chars().last()? {
        'k' | 'K' => (&cell[..cell.len() - 1], 1e3),
        'm' | 'M' => (&cell[..cell.len() - 1], 1e6),
        _ => (cell, 1.0),
    };
    let value: f64 = number.trim().parse().ok()?;
    Some((value * factor).round() as u64)
}

/// A fresh empty directory for the configuration-only probe; removed by [`EmptyDir`]'s drop.
pub struct EmptyDir(pub PathBuf);

impl EmptyDir {
    pub fn create() -> std::io::Result<EmptyDir> {
        let dir = std::env::temp_dir().join(format!("agentmeter-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(EmptyDir(dir))
    }
}

impl Drop for EmptyDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r###"{"is_error":false,"result":"## Context Usage\n\n**Model:** claude-opus-5[1m]  \n**Tokens:** 33.9k / 1m (3%)\n\n### Estimated usage by category\n\n| Category | Tokens | Percentage |\n|----------|--------|------------|\n| System prompt | 3k | 0.3% |\n| System tools | 19.1k | 1.9% |\n| MCP tools (deferred) | 11.9k | 1.2% |\n| Memory files | 1.4k | 0.1% |\n| Skills | 9.9k | 1.0% |\n| Custom agents | 514 | 0.1% |\n| Free space | 933.1k | 93.3% |\n| Autocompact buffer | 33k | 3.3% |\n\n### MCP Tools\n\n| Tool | Server | Tokens |\n|------|--------|--------|\n| mcp__a__x | a | 1.1k |\n| mcp__a__y | a | 400 |\n| mcp__b__z | b | 120 |\n\n### Memory Files\n\n| Type | Path | Tokens |\n|------|------|--------|\n| User | /home/u/.claude/CLAUDE.md | 1.4k |\n\n### Skills\n\n| Skill | Source | Tokens |\n|-------|--------|--------|\n| s | User | ~70 |\n"}"###;

    #[test]
    fn reads_the_tables_and_leaves_out_free_space_and_deferred() {
        let p = parse(ANSWER, "empty directory", "/tmp/x").unwrap();
        assert_eq!(p.model.as_deref(), Some("claude-opus-5[1m]"));
        assert_eq!(p.total, 3_000 + 19_100 + 1_400 + 9_900 + 514);
        assert_eq!(p.sent[0].label, "System tools");
        assert_eq!(p.deferred.len(), 1);
        assert_eq!(p.memory_files[0].tokens, 1_400);
        assert_eq!(p.mcp_servers[0].label, "a");
        assert_eq!(p.mcp_servers[0].tokens, 1_500);
    }

    #[test]
    fn token_cells() {
        assert_eq!(parse_tokens("19.1k"), Some(19_100));
        assert_eq!(parse_tokens("~70"), Some(70));
        assert_eq!(parse_tokens("< 20"), Some(20));
        assert_eq!(parse_tokens("1.2m"), Some(1_200_000));
        assert_eq!(parse_tokens("Tokens"), None);
    }

    #[test]
    fn an_error_answer_or_a_changed_format_is_a_failure() {
        assert!(parse(r#"{"is_error":true,"result":"x"}"#, "p", "/").is_err());
        assert!(parse(r#"{"is_error":false,"result":"no tables"}"#, "p", "/").is_err());
        assert!(parse("not json", "p", "/").is_err());
    }
}
