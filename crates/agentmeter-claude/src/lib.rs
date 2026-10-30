//! Claude Code adapter: reads `~/.claude/projects/**/*.jsonl` (including `subagents/`) into a
//! [`Dataset`].
//!
//! The log format is not documented and changes between versions. Reading is therefore two-stage
//! and tolerant: a weakly typed raw stage without `deny_unknown_fields`, then the normalised
//! [`Request`] and [`Compaction`]. Lines that cannot be read and entry types or usage fields
//! that are not known are counted in [`ReadStats`], never fatal. Only usage fields are read;
//! message content is skipped without being parsed.

pub mod probe;
mod raw;

use agentmeter_core::model::{Compaction, Dataset, ReadStats, Request, SessionStart};
use agentmeter_core::time::parse_iso_ms;
use raw::{RawCompact, RawLine, RawMessage};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Entry types that carry no usage and are expected in the logs. Anything else is counted as
/// unknown so format drift shows up in the report.
const KNOWN_TYPES: &[&str] = &[
    "user",
    "assistant",
    "system",
    "attachment",
    "summary",
    "progress",
    "queue-operation",
    "file-history-snapshot",
    "file-history-delta",
    "custom-title",
    "ai-title",
    "last-prompt",
    "agent-name",
    "agent-color",
    "mode",
    "api-request",
    "api-request-shape",
    "api-request-blob",
    "atis-latch",
    "relocated",
    "cost-state",
];

/// Fields of `usage` that are known; the token counts are read, the rest is expected and unused.
const KNOWN_USAGE_FIELDS: &[&str] = &[
    "output_tokens_details",
    "server_tool_use",
    "service_tier",
    "inference_geo",
    "iterations",
    "speed",
    "fallback_credit",
];

/// Configuration directories to read: `CLAUDE_CONFIG_DIR` (comma separated), else `~/.claude`.
pub fn config_dirs() -> Vec<PathBuf> {
    if let Some(value) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return value
            .to_string_lossy()
            .split(',')
            .map(|part| PathBuf::from(part.trim()))
            .collect();
    }
    std::env::var_os("HOME")
        .map(|home| vec![PathBuf::from(home).join(".claude")])
        .unwrap_or_default()
}

/// Reads the `projects/` directory of every configuration directory. Directories that do not
/// exist are skipped; the caller can tell from `stats.files`.
pub fn read(config_dirs: &[PathBuf]) -> Dataset {
    let mut reader = Reader::default();
    for dir in config_dirs {
        let projects = dir.join("projects");
        let mut files = Vec::new();
        collect_files(&projects, &mut files);
        for file in files {
            reader.read_file(&projects, &file);
        }
    }
    reader.finish()
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect_files(&path, out),
            Ok(kind) if kind.is_file() && path.extension() == Some(OsStr::new("jsonl")) => {
                out.push(path);
            }
            _ => {}
        }
    }
}

/// Sum used to pick the most complete of several copies of one response: a streaming snapshot
/// carries partial output tokens, the final line the full count.
fn token_sum(r: &Request) -> u64 {
    r.context() + r.output
}

struct Entry {
    request: Request,
    message_id: String,
}

#[derive(Default)]
struct Reader {
    stats: ReadStats,
    /// Best copy per (message id, request id).
    by_key: HashMap<(String, String), Entry>,
    /// Requests without a message id cannot be matched.
    keyless: Vec<Request>,
    compactions: Vec<Compaction>,
    files: Vec<String>,
    session_starts: Vec<SessionStart>,
    source: usize,
}

/// What one file showed about how it begins.
#[derive(Default)]
struct FileFacts {
    usage_lines: u64,
    /// `parentUuid` of the first user or assistant line; `None` until one was seen.
    first_message_is_root: Option<bool>,
    /// Lines carry a session id other than the file name: resume or fork copied history.
    foreign_session: bool,
    /// `entrypoint` and `cwd` of the first line that has them.
    entrypoint: Option<String>,
    cwd: Option<String>,
}

impl Reader {
    fn read_file(&mut self, projects: &Path, path: &Path) {
        let Ok(file) = File::open(path) else {
            return;
        };
        self.stats.files += 1;
        let source = self.source;
        self.source += 1;

        let relative = path.strip_prefix(projects).unwrap_or(path);
        // The project is the directory under `projects/`, never the working directory in the
        // lines: sub-agents and worktrees run in directories of their own.
        let project = relative
            .iter()
            .next()
            .filter(|_| relative.components().count() > 1)
            .map_or_else(
                || "unknown".to_owned(),
                |c| project_name(&c.to_string_lossy()),
            );
        self.files.push(relative.to_string_lossy().into_owned());
        let in_subagents = relative.iter().any(|c| c == "subagents");
        let file_stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());

        let mut facts = FileFacts::default();
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if buffer.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            self.stats.lines += 1;
            let Ok(line) = serde_json::from_slice::<RawLine>(&buffer) else {
                self.stats.skipped_lines += 1;
                continue;
            };
            if let Some(version) = line.version.as_deref() {
                self.stats.note_version(version);
            }
            let session = line
                .session_id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| file_stem.clone());
            let project = project.clone();
            let sub = in_subagents || line.is_sidechain.unwrap_or(false);
            if !in_subagents {
                if matches!(line.kind.as_deref(), Some("user" | "assistant")) {
                    facts
                        .first_message_is_root
                        .get_or_insert(line.parent_uuid.is_none());
                }
                if facts.entrypoint.is_none() {
                    facts.entrypoint = line.entrypoint.clone().filter(|e| !e.is_empty());
                }
                if facts.cwd.is_none() {
                    facts.cwd = line.cwd.clone().filter(|c| !c.is_empty());
                }
                if line
                    .session_id
                    .as_deref()
                    .is_some_and(|s| !s.is_empty() && s != file_stem)
                {
                    facts.foreign_session = true;
                }
            }

            match line.kind.as_deref() {
                Some("assistant") => {
                    self.assistant(&line, session, project, source, sub, &mut facts);
                }
                Some("system") => self.system(&line, session, project, source),
                Some(kind) if KNOWN_TYPES.contains(&kind) => {}
                Some(kind) => *self.stats.unknown_types.entry(kind.to_owned()).or_default() += 1,
                None => {
                    *self
                        .stats
                        .unknown_types
                        .entry("(none)".to_owned())
                        .or_default() += 1
                }
            }
        }
        self.stats.files_without_usage += u64::from(facts.usage_lines == 0);
        if !in_subagents {
            let reason = if facts.foreign_session {
                Some("inherited-history")
            } else if facts.first_message_is_root != Some(true) {
                Some("no-log-start")
            } else {
                None
            };
            self.session_starts.push(SessionStart {
                session: file_stem,
                source,
                clean: reason.is_none(),
                reason,
                entrypoint: facts.entrypoint,
                cwd: facts.cwd,
            });
        }
    }

    fn assistant(
        &mut self,
        line: &RawLine,
        session: String,
        project: String,
        source: usize,
        sub: bool,
        facts: &mut FileFacts,
    ) {
        let Some(message) = line
            .message
            .and_then(|raw| serde_json::from_str::<RawMessage>(raw.get()).ok())
        else {
            self.stats.skipped_lines += 1;
            return;
        };
        let Some(usage) = message.usage else {
            return;
        };
        let Some(time_ms) = line.timestamp.as_deref().and_then(parse_iso_ms) else {
            self.stats.skipped_lines += 1;
            return;
        };
        facts.usage_lines += 1;
        for field in usage.rest.keys() {
            if !KNOWN_USAGE_FIELDS.contains(&field.as_str()) {
                *self
                    .stats
                    .unknown_usage_fields
                    .entry(field.clone())
                    .or_default() += 1;
            }
        }

        // The nested object splits cache writes by lifetime. Claude Code before 2.1.152
        // filled only the flat total, later versions may fill only the nested object.
        let nested = usage
            .cache_creation
            .map(|c| (c.five_minutes.unwrap_or(0), c.one_hour.unwrap_or(0)));
        let flat = usage.cache_creation_input_tokens.unwrap_or(0);
        let split = nested.is_some_and(|(five, hour)| five + hour > 0);
        let (write_5m, write_1h) = match nested {
            Some((five, hour)) if split => (five, hour),
            _ => (flat, 0),
        };
        let request = Request {
            session,
            project,
            source,
            message_id: message.id.clone().unwrap_or_default(),
            time_ms,
            model: message.model.unwrap_or_default(),
            sub,
            fresh: usage.input_tokens.unwrap_or(0),
            cache_read: usage.cache_read_input_tokens.unwrap_or(0),
            cache_write_5m: write_5m,
            cache_write_1h: write_1h,
            output: usage.output_tokens.unwrap_or(0),
            window: None,
        };
        // Placeholder lines without any usage (synthetic messages) are not requests.
        if token_sum(&request) == 0 {
            return;
        }
        if split {
            self.stats.requests_with_ttl_split += 1;
        } else {
            self.stats.requests_flat_cache += 1;
        }
        let Some(id) = message.id.filter(|id| !id.is_empty()) else {
            self.keyless.push(request);
            return;
        };
        let key = (id.clone(), line.request_id.clone().unwrap_or_default());
        match self.by_key.get_mut(&key) {
            Some(known) => {
                self.stats.duplicates_merged += 1;
                // The main conversation wins over a sub-agent copy, then the larger total.
                let better = (!request.sub, token_sum(&request))
                    > (!known.request.sub, token_sum(&known.request));
                if better {
                    known.request = request;
                }
            }
            None => {
                self.by_key.insert(
                    key,
                    Entry {
                        request,
                        message_id: id,
                    },
                );
            }
        }
    }

    fn system(&mut self, line: &RawLine, session: String, project: String, source: usize) {
        let Some(raw) = line.compact_metadata else {
            return;
        };
        let Ok(meta) = serde_json::from_str::<RawCompact>(raw.get()) else {
            self.stats.skipped_lines += 1;
            return;
        };
        let (Some(before), Some(time_ms)) = (
            meta.pre_tokens,
            line.timestamp.as_deref().and_then(parse_iso_ms),
        ) else {
            self.stats.skipped_lines += 1;
            return;
        };
        self.stats.compactions_without_post_tokens += u64::from(meta.post_tokens.is_none());
        self.compactions.push(Compaction {
            session,
            project,
            source,
            event_id: line.uuid.clone().unwrap_or_default(),
            time_ms,
            tokens_before: before,
            tokens_after: meta.post_tokens,
            auto: Some(meta.trigger.as_deref() != Some("manual")),
        });
    }

    fn finish(mut self) -> Dataset {
        // A sub-agent line that repeats a message of the main conversation (same message id,
        // another request id) is a replay of the parent history, not a request of its own.
        let main_ids: HashSet<&str> = self
            .by_key
            .values()
            .filter(|e| !e.request.sub)
            .map(|e| e.message_id.as_str())
            .collect();
        let replays: Vec<(String, String)> = self
            .by_key
            .iter()
            .filter(|(_, e)| e.request.sub && main_ids.contains(e.message_id.as_str()))
            .map(|(key, _)| key.clone())
            .collect();
        self.stats.replays_dropped = replays.len() as u64;
        for key in &replays {
            self.by_key.remove(key);
        }

        let mut requests: Vec<Request> = self.by_key.into_values().map(|e| e.request).collect();
        requests.extend(self.keyless);
        requests.sort_by(|a, b| {
            (a.time_ms, &a.session, a.source, a.output)
                .cmp(&(b.time_ms, &b.session, b.source, b.output))
        });

        // The same boundary can appear in the files of a session and its sub-agents.
        let mut compactions = self.compactions;
        compactions.sort_by(|a, b| {
            (a.time_ms, &a.session, a.tokens_before).cmp(&(b.time_ms, &b.session, b.tokens_before))
        });
        compactions.dedup_by(|a, b| {
            a.time_ms == b.time_ms && a.session == b.session && a.tokens_before == b.tokens_before
        });

        self.stats.cache_write_measured = !requests.is_empty();
        self.stats.requests_without_model =
            requests.iter().filter(|r| r.model.is_empty()).count() as u64;
        Dataset {
            requests,
            compactions,
            files: self.files,
            session_starts: self.session_starts,
            stats: self.stats,
        }
    }
}

/// Display name of a project directory such as `-Users-me-GitHub-demo-app`. The encoded path is
/// ambiguous where the name itself holds a hyphen, so this stays simple: a worktree directory
/// (`<project>--claude-worktrees-<name>`) folds into its parent project, then everything up to
/// `-GitHub-` is dropped, otherwise only the leading hyphen.
fn project_name(directory: &str) -> String {
    let parent = directory
        .split_once("--claude-worktrees-")
        .map_or(directory, |(before, _)| before);
    let name = parent
        .split_once("-GitHub-")
        .map_or_else(|| parent.trim_start_matches('-'), |(_, rest)| rest);
    if name.is_empty() { "unknown" } else { name }.to_owned()
}
