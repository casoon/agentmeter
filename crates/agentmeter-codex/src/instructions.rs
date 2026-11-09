//! `--instructions`: the `AGENTS.md` files Codex CLI loads for a directory, found by its loading
//! rules without starting Codex. Not the rules of Claude Code: nothing is shared with it.
//!
//! Rules followed (Codex `agents_md.rs` and `config.toml` keys, as of rust-v0.165, 2026-11):
//! - Global: `AGENTS.override.md`, else `AGENTS.md` in the Codex home.
//! - Project: from the project root (nearest ancestor with `.git`, a directory or a worktree's
//!   file; without one only the directory itself) down to the directory, one file per directory:
//!   `AGENTS.override.md`, else `AGENTS.md`, else the first existing `project_doc_fallback_filenames`.
//! - The project files are concatenated up to `project_doc_max_bytes` (default 32 KiB); the rest
//!   is cut off.
//! - Codex skips project files of projects it does not trust; trust is not evaluated here.
//! - Subdirectories below the directory are not loaded.

use agentmeter_core::instructions::{InstructionFile, InstructionScan, LoadState};
use std::fs;
use std::path::{Path, PathBuf};

pub const RULES_VERSION: &str = "Codex CLI AGENTS.md discovery (rust-v0.165), 2026-11";
/// Default of `project_doc_max_bytes`.
const DEFAULT_MAX_BYTES: u64 = 32 * 1024;
const OVERRIDE: &str = "AGENTS.override.md";
const DEFAULT: &str = "AGENTS.md";

/// Scans `cwd`. `ceiling` stops the search for the project root after that directory (tests).
pub fn scan(cwd: &Path, homes: &[PathBuf], ceiling: Option<&Path>) -> InstructionScan {
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut files = Vec::new();
    let mut notes = Vec::new();
    let mut max_bytes = DEFAULT_MAX_BYTES;
    let mut fallbacks: Vec<String> = Vec::new();

    for home in homes {
        let config = read_config(&home.join("config.toml"));
        if let Some(n) = config.max_bytes {
            max_bytes = n;
        }
        fallbacks.extend(config.fallbacks);
        pick(home, &[OVERRIDE, DEFAULT], "user", &mut files);
    }

    // Project root: nearest ancestor holding `.git`.
    let mut root = None;
    let mut dir = Some(cwd.as_path());
    while let Some(d) = dir {
        if d.join(".git").exists() {
            root = Some(d);
            break;
        }
        if ceiling == Some(d) {
            break;
        }
        dir = d.parent();
    }
    let mut chain: Vec<&Path> = Vec::new();
    let mut d = Some(cwd.as_path());
    while let Some(p) = d {
        chain.push(p);
        if Some(p) == root || root.is_none() {
            break;
        }
        d = p.parent();
    }
    chain.reverse();
    if root.is_none() {
        notes.push("no .git found above: only the directory itself is searched".to_owned());
    }
    let mut names = vec![OVERRIDE.to_owned(), DEFAULT.to_owned()];
    for n in fallbacks {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let first_project = files.len();
    for p in chain {
        let scope = if p == cwd { "project" } else { "ancestor" };
        pick(p, &names, scope, &mut files);
    }

    // The byte budget runs over the project files in order.
    let mut remaining = max_bytes;
    let mut cut = false;
    for file in files[first_project..]
        .iter_mut()
        .filter(|f| f.state == LoadState::Expected)
    {
        if remaining >= file.bytes {
            remaining -= file.bytes;
        } else if remaining > 0 {
            file.loaded_bytes = remaining;
            file.note = Some(format!(
                "cut after {remaining} of {} bytes by project_doc_max_bytes ({max_bytes})",
                file.bytes
            ));
            remaining = 0;
            cut = true;
        } else {
            file.state = LoadState::NotLoaded;
            file.loaded_bytes = 0;
            file.note = Some(format!("beyond project_doc_max_bytes ({max_bytes})"));
            cut = true;
        }
    }
    notes.push(
        "Codex skips project files of projects it does not trust (config.toml [projects]); trust \
         is not evaluated here, so project files are 'expected'."
            .to_owned(),
    );
    notes.push("Files in subdirectories below the directory are not loaded by Codex.".to_owned());
    let mut scan = InstructionScan::new(
        "codex",
        RULES_VERSION,
        cwd.display().to_string(),
        files,
        notes,
    )
    .finish();
    if cut {
        let total: u64 = scan.files[first_project..]
            .iter()
            .filter(|f| f.state != LoadState::Unreadable)
            .map(|f| f.bytes)
            .sum();
        scan.findings.push(agentmeter_core::instructions::InstructionFinding {
            rule: "load-limit",
            file: scan.dir.clone(),
            measure: format!(
                "project files hold {total} bytes, project_doc_max_bytes is {max_bytes}: the rest is not loaded"
            ),
            certainty: "high",
            check: "Shorten the files or raise project_doc_max_bytes in config.toml; the cut \
                    follows the order root to directory."
                .to_owned(),
        });
    }
    scan
}

/// In one directory: the first existing name loads, the others shadowed by it are listed as
/// not loaded.
fn pick(dir: &Path, names: &[&str], scope: &'static str, files: &mut Vec<InstructionFile>) {
    let mut winner: Option<String> = None;
    for name in names {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let key = path.display().to_string();
        let state = if winner.is_none() {
            LoadState::Expected
        } else {
            LoadState::NotLoaded
        };
        let mut file = InstructionFile::new(key.clone(), scope, state);
        match fs::read(&path) {
            Ok(content) => file = file.with_content(&content),
            Err(e) => {
                file.state = LoadState::Unreadable;
                file.note = Some(format!("could not be read: {e}"));
            }
        }
        if let Some(w) = &winner {
            file.note = Some(format!("shadowed by {w} in the same directory"));
        } else if file.state == LoadState::Expected {
            winner = Some((*name).to_owned());
        }
        files.push(file);
    }
}

#[derive(Default)]
struct Config {
    max_bytes: Option<u64>,
    fallbacks: Vec<String>,
}

/// The two top-level keys that matter, read from `config.toml` without a TOML parser; a table
/// header ends the top level (profile settings are not evaluated).
fn read_config(path: &Path) -> Config {
    let mut config = Config::default();
    let Ok(text) = fs::read_to_string(path) else {
        return config;
    };
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.split('#').next().unwrap_or("").trim();
        match key.trim() {
            "project_doc_max_bytes" => config.max_bytes = value.replace('_', "").parse().ok(),
            "project_doc_fallback_filenames" => {
                config.fallbacks = value
                    .trim_matches(['[', ']'])
                    .split(',')
                    .map(|s| s.trim().trim_matches(['"', '\'']).to_owned())
                    .filter(|s| !s.is_empty() && !s.contains('/'))
                    .collect();
            }
            _ => {}
        }
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_two_keys_at_top_level_only() {
        let dir = std::env::temp_dir().join(format!("agentmeter-codex-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        fs::write(
            &file,
            "project_doc_max_bytes = 1_000 # x\nproject_doc_fallback_filenames = [\"A.md\", \"B.md\"]\n[profiles.x]\nproject_doc_max_bytes = 5\n",
        )
        .unwrap();
        let c = read_config(&file);
        assert_eq!(c.max_bytes, Some(1000));
        assert_eq!(c.fallbacks, ["A.md", "B.md"]);
        let _ = fs::remove_dir_all(dir);
    }
}
