//! Claude Code adapter: reads `~/.claude/projects/**/*.jsonl` (including `subagents/`) into a
//! [`Dataset`].
//!
//! The log format is not documented and changes between versions. Reading is therefore two-stage
//! and tolerant: a weakly typed raw stage without `deny_unknown_fields`, then the normalised
//! [`Request`] and [`Compaction`]. Lines that cannot be read and entry types or usage fields
//! that are not known are counted in [`ReadStats`], never fatal. Only usage fields are read;
//! message content is skipped without being parsed.

pub mod instructions;
pub mod probe;
mod raw;
pub mod settings;
pub mod skills;

use agentmeter_core::model::{
    AgentLink, AgentRun, Compaction, Dataset, ReadStats, Request, SessionStart,
};
use agentmeter_core::time::parse_iso_ms;
use agentmeter_core::toolcalls::{ToolData, ToolEvent, ToolKind, ToolStats, ToolStatus};
use raw::{
    RawAgentResult, RawAttachment, RawBlock, RawCompact, RawLine, RawMessage, RawRunUsage,
    RawToolMessage, ToolContent,
};
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
    read_with_tools(config_dirs, false).0
}

/// Like [`read`]; with `tools` also the tool calls (`--tool-calls`), for which message content
/// is opened. Without it content stays unread, as in the default mode.
pub fn read_with_tools(config_dirs: &[PathBuf], tools: bool) -> (Dataset, ToolData) {
    read_with_progress(config_dirs, tools, &mut |_, _| {})
}

/// Like [`read_with_tools`]; `progress` is called with the files read so far and the number of
/// files in all, once before the first file and once after each.
pub fn read_with_progress(
    config_dirs: &[PathBuf],
    tools: bool,
    progress: &mut dyn FnMut(u64, u64),
) -> (Dataset, ToolData) {
    let mut reader = Reader {
        collect_tools: tools,
        ..Reader::default()
    };
    let per_dir: Vec<(PathBuf, Vec<PathBuf>)> = config_dirs
        .iter()
        .map(|dir| {
            let projects = dir.join("projects");
            let mut files = Vec::new();
            collect_files(&projects, &mut files);
            (projects, files)
        })
        .collect();
    let total = per_dir.iter().map(|(_, files)| files.len() as u64).sum();
    let mut done = 0;
    progress(done, total);
    for (projects, files) in &per_dir {
        for file in files {
            reader.read_file(projects, file);
            done += 1;
            progress(done, total);
        }
    }
    let tool_data = reader.tool_data();
    (reader.finish(), tool_data)
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
    agent_links: Vec<AgentLink>,
    /// Finished agent runs by (agent, end, duration): a resumed session copies the records.
    agent_runs: HashSet<(Option<String>, i64, u64)>,
    source: usize,
    /// `--tool-calls`: open message content for tool calls and results.
    collect_tools: bool,
    calls: HashMap<String, ToolEvent>,
    results: HashMap<String, (ToolStatus, Option<u64>)>,
    tool_seq: u64,
    tool_duplicates: u64,
}

/// What one file showed about how it begins.
#[derive(Default)]
struct FileFacts {
    usage_lines: u64,
    /// `parentUuid` of the first user or assistant line; `None` until one was seen.
    first_message_is_root: Option<bool>,
    /// Lines carry a session id other than the file name: resume or fork copied history.
    foreign_session: bool,
    /// The first session id other than the file name: the session a resume copied from.
    parent: Option<String>,
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
                    if facts.parent.is_none() {
                        facts.parent = line.session_id.clone();
                    }
                }
            }

            match line.kind.as_deref() {
                Some("assistant") => {
                    if self.collect_tools {
                        self.tool_uses(&line, &session, &project, source, sub);
                    }
                    self.assistant(&line, session, project, source, sub, &mut facts);
                }
                Some("user") => {
                    self.agent_result(&line);
                    if self.collect_tools {
                        self.tool_results(&line);
                    }
                }
                Some("attachment") => self.run_notice(&line),
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
        if in_subagents {
            // `<project>/<session>/subagents/<agent>.jsonl`: the session folder names the parent.
            let parts: Vec<_> = relative.iter().collect();
            let parent = parts
                .iter()
                .position(|c| *c == "subagents")
                .filter(|at| *at >= 2)
                .map(|at| parts[at - 1].to_string_lossy().into_owned());
            self.agent_links.push(AgentLink {
                source,
                agent: file_stem,
                basis: parent.as_ref().map(|_| "session_folder"),
                parent,
            });
        } else {
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
                parent: facts.parent,
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

    /// `tool_use` blocks of an assistant line. Streaming copies and sub-agent replays repeat a
    /// call id: the main conversation's copy wins, else the first.
    fn tool_uses(
        &mut self,
        line: &RawLine,
        session: &str,
        project: &str,
        source: usize,
        sub: bool,
    ) {
        let (Some(raw), Some(time_ms)) = (
            line.message,
            line.timestamp.as_deref().and_then(parse_iso_ms),
        ) else {
            return;
        };
        let Ok(RawToolMessage {
            content: ToolContent::Blocks(blocks),
        }) = serde_json::from_str::<RawToolMessage>(raw.get())
        else {
            return;
        };
        for block in blocks {
            if block.kind.as_deref() != Some("tool_use") {
                continue;
            }
            let (Some(id), Some(name)) = (block.id.clone(), block.name.clone()) else {
                continue;
            };
            self.tool_seq += 1;
            let event = tool_event(
                &block,
                id.clone(),
                name,
                session,
                project,
                source,
                sub,
                self.tool_seq,
                time_ms,
            );
            match self.calls.get(&id) {
                Some(known) => {
                    self.tool_duplicates += 1;
                    if known.sub && !event.sub {
                        self.calls.insert(id, event);
                    }
                }
                None => {
                    self.calls.insert(id, event);
                }
            }
        }
    }

    /// The result of an agent that ran in the foreground: its id and the figures of the run.
    fn agent_result(&mut self, line: &RawLine) {
        let Some(raw) = line
            .tool_use_result
            .filter(|r| r.get().contains("\"totalDurationMs\""))
        else {
            return;
        };
        let Ok(RawAgentResult {
            agent_id: Some(id),
            total_duration_ms: Some(duration_ms),
        }) = serde_json::from_str::<RawAgentResult>(raw.get())
        else {
            return;
        };
        let Some(end_ms) = line.timestamp.as_deref().and_then(parse_iso_ms) else {
            return;
        };
        self.agent_runs
            .insert((Some(agent_name(&id)), end_ms, duration_ms));
    }

    /// The notice that a background agent finished. Its figures are structured; the agent's id
    /// stands only in the notice text, which is searched for it when tool calls are read and
    /// otherwise left unopened.
    fn run_notice(&mut self, line: &RawLine) {
        let Some(raw) = line.attachment else {
            return;
        };
        let Ok(a) = serde_json::from_str::<RawAttachment>(raw.get()) else {
            return;
        };
        if a.kind.as_deref() != Some("queued_command")
            || a.command_mode.as_deref() != Some("task-notification")
        {
            return;
        }
        let Some(RawRunUsage {
            duration_ms: Some(duration_ms),
        }) = a.usage
        else {
            return;
        };
        let Some(end_ms) = line
            .timestamp
            .as_deref()
            .or(a.timestamp.as_deref())
            .and_then(parse_iso_ms)
        else {
            return;
        };
        let agent = a
            .prompt
            .filter(|_| self.collect_tools)
            .and_then(|p| task_id(p.get()))
            .map(agent_name);
        self.agent_runs.insert((agent, end_ms, duration_ms));
    }

    /// `tool_result` blocks of a user line: status and size only.
    fn tool_results(&mut self, line: &RawLine) {
        let Some(raw) = line.message else {
            return;
        };
        let Ok(RawToolMessage {
            content: ToolContent::Blocks(blocks),
        }) = serde_json::from_str::<RawToolMessage>(raw.get())
        else {
            return;
        };
        for block in blocks {
            if block.kind.as_deref() != Some("tool_result") {
                continue;
            }
            let Some(id) = block.tool_use_id.clone() else {
                continue;
            };
            let status = match block.is_error {
                Some(true) => ToolStatus::Error,
                Some(false) => ToolStatus::Ok,
                None => ToolStatus::Unknown,
            };
            let bytes = block.content.as_ref().map(raw::ResultContent::text_bytes);
            self.results.entry(id).or_insert((status, bytes));
        }
    }

    /// Joins calls and results; a result whose call is not in the logs is counted, not used.
    fn tool_data(&self) -> ToolData {
        let mut stats = ToolStats {
            duplicates_merged: self.tool_duplicates,
            ..ToolStats::default()
        };
        let mut events: Vec<ToolEvent> = self.calls.values().cloned().collect();
        for e in &mut events {
            if let Some((status, bytes)) = self.results.get(&e.call_id) {
                e.status = *status;
                e.output_bytes = *bytes;
                e.has_result = true;
            }
            *stats.tools.entry(e.tool.clone()).or_default() += 1;
        }
        stats.results_without_call = self
            .results
            .keys()
            .filter(|id| !self.calls.contains_key(*id))
            .count() as u64;
        stats.calls = events.len() as u64;
        stats.calls_without_result = events.iter().filter(|e| !e.has_result).count() as u64;
        events.sort_by_key(|e| (e.time_ms, e.source, e.seq));
        ToolData {
            events,
            stats,
            has_targets: true,
            has_status: true,
            has_sizes: true,
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
        let mut agent_runs = Vec::new();
        let mut agent_runs_unlinked = 0;
        for (agent, end_ms, duration_ms) in self.agent_runs {
            match agent {
                Some(agent) => agent_runs.push(AgentRun {
                    agent,
                    end_ms,
                    duration_ms,
                }),
                None => agent_runs_unlinked += 1,
            }
        }
        agent_runs.sort_by(|a, b| (a.end_ms, &a.agent).cmp(&(b.end_ms, &b.agent)));
        Dataset {
            requests,
            compactions,
            files: self.files,
            session_starts: self.session_starts,
            agent_links: self.agent_links,
            agent_runs,
            agent_runs_unlinked,
            stats: self.stats,
        }
    }
}

/// The name of an agent's log file for the id the log gives in a result or notice.
fn agent_name(id: &str) -> String {
    if id.starts_with("agent-") {
        id.to_owned()
    } else {
        format!("agent-{id}")
    }
}

/// The id between `<task-id>` and `</task-id>` in a notice; nothing else of the text is kept.
fn task_id(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once("<task-id>")?;
    let (id, _) = rest.split_once("</task-id>")?;
    (!id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(id)
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

/// One call as a [`ToolEvent`]. File tools keep their path (and line range for reads); every
/// other call keeps only a hash of what makes it identical. The command text is not kept.
#[allow(clippy::too_many_arguments)]
fn tool_event(
    block: &RawBlock,
    call_id: String,
    tool: String,
    session: &str,
    project: &str,
    source: usize,
    sub: bool,
    seq: u64,
    time_ms: i64,
) -> ToolEvent {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let input = block.input.as_ref();
    let text = |key: &str| input.and_then(|i| i.get(key)).and_then(|v| v.as_str());
    let number = |key: &str| {
        input
            .and_then(|i| i.get(key))
            .and_then(serde_json::Value::as_u64)
    };
    let (kind, target, range, hashed) = match tool.as_str() {
        "Read" => {
            let range = match (number("offset"), number("limit")) {
                (None, None) => None,
                (offset, limit) => Some((offset.unwrap_or(0), limit.unwrap_or(u64::MAX))),
            };
            (
                ToolKind::Read,
                text("file_path").map(str::to_owned),
                range,
                None,
            )
        }
        "Edit" | "Write" | "MultiEdit" => (
            ToolKind::Write,
            text("file_path").map(str::to_owned),
            None,
            None,
        ),
        "NotebookEdit" => (
            ToolKind::Write,
            text("notebook_path").map(str::to_owned),
            None,
            None,
        ),
        // The skill name is kept as the target; the arguments only go into the fingerprint.
        "Skill" => (
            ToolKind::Other,
            text("skill").map(str::to_owned),
            None,
            input.map(|i| serde_json::Value::Object(i.clone()).to_string()),
        ),
        "Bash" => (
            ToolKind::Command,
            None,
            None,
            text("command").map(str::to_owned),
        ),
        _ => (
            ToolKind::Other,
            None,
            None,
            input.map(|i| serde_json::Value::Object(i.clone()).to_string()),
        ),
    };
    let fingerprint = hashed.map(|h| {
        let mut hasher = DefaultHasher::new();
        h.trim().hash(&mut hasher);
        hasher.finish()
    });
    ToolEvent {
        source,
        session: session.to_owned(),
        project: project.to_owned(),
        sub,
        seq,
        time_ms,
        call_id,
        tool,
        kind,
        target,
        range,
        fingerprint,
        status: ToolStatus::Unknown,
        output_bytes: None,
        has_result: false,
    }
}
