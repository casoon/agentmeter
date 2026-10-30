//! The schema every adapter produces.

use serde::Serialize;
use std::collections::BTreeMap;

/// One model request with its token usage. Times are milliseconds since the Unix epoch, UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Request {
    pub session: String,
    pub project: String,
    /// Index of the log file the request came from; requests of one file form one stream.
    pub source: usize,
    /// Message id of the response; empty when the line has none. Evidence reference, no content.
    pub message_id: String,
    pub time_ms: i64,
    pub model: String,
    /// Written by a sub-agent.
    pub sub: bool,
    /// Input tokens that were neither read from nor written to the cache.
    pub fresh: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    /// Context window of the model as the log states it (Codex); `None` when not recorded.
    pub window: Option<u64>,
}

impl Request {
    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m + self.cache_write_1h
    }

    /// What the request sent to the model: fresh input plus everything from the cache.
    pub fn context(&self) -> u64 {
        self.fresh + self.cache_read + self.cache_write()
    }
}

/// One context compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Compaction {
    pub session: String,
    pub project: String,
    pub source: usize,
    /// Event id (`uuid`) of the boundary line; empty when absent.
    pub event_id: String,
    pub time_ms: i64,
    pub tokens_before: u64,
    pub tokens_after: Option<u64>,
    /// Triggered by the agent when the window filled up (`true`) or by the user (`false`);
    /// `None` when the log does not say.
    pub auto: Option<bool>,
}

/// What reading the logs saw, including what could not be used. Nothing is dropped silently.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ReadStats {
    pub files: u64,
    pub lines: u64,
    /// Lines that were not valid JSON or lacked a field a usage line needs.
    pub skipped_lines: u64,
    /// Entry types the adapter does not know, with their line counts.
    pub unknown_types: BTreeMap<String, u64>,
    /// Fields inside `usage` the adapter does not know, with their line counts.
    pub unknown_usage_fields: BTreeMap<String, u64>,
    /// Streaming snapshots and copies of the same response that were merged into one request.
    pub duplicates_merged: u64,
    /// Sub-agent lines that replay a parent message and were dropped.
    pub replays_dropped: u64,
    /// Usage lines (before merging duplicates) whose cache writes were split by lifetime.
    pub requests_with_ttl_split: u64,
    /// Usage lines that only have the flat cache-write total.
    pub requests_flat_cache: u64,
    /// Files that contained no line with usage.
    pub files_without_usage: u64,
    /// Compressed log files (`.zst`) that were read.
    pub compressed_files_read: u64,
    /// Compressed log files that could not be decoded (damaged, over the window or size limit);
    /// what was read before the damage is kept.
    pub compressed_files_unreadable: u64,
    /// Compressed files not read because the same rollout exists in plain text.
    pub compressed_files_shadowed: u64,
    /// The log records cache writes (non-zero values seen); without them cache-write rules are
    /// not measurable, and a zero would be a guess.
    pub cache_write_measured: bool,
    /// Events whose change of a cumulative total differs from the event's own per-request value,
    /// or whose cumulative total went backwards.
    pub usage_delta_mismatches: u64,
    /// Requests without a model name: no price or model-specific rule can apply.
    pub requests_without_model: u64,
    /// Compactions without the size after the compaction (`postTokens`).
    pub compactions_without_post_tokens: u64,
    /// Entries whose version is the placeholder `0.0.0`; left out of the version range.
    pub placeholder_version_entries: u64,
    pub first_version: Option<String>,
    pub last_version: Option<String>,
}

/// How a session's log file begins, as far as the file itself shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionStart {
    /// Session id taken from the file name.
    pub session: String,
    pub source: usize,
    /// The file starts at a conversation root and holds no lines of another session.
    pub clean: bool,
    /// Why not: `inherited-history` (lines of another session: resume or fork) or
    /// `no-log-start` (the first message has a parent that is not in the file).
    pub reason: Option<&'static str>,
    /// The front end that wrote the session as the log names it (`claude-desktop`, `cli`,
    /// `Codex Desktop`, ...); `None` when not recorded.
    pub entrypoint: Option<String>,
    /// Working directory at the start of the session; used only to run a probe there.
    pub cwd: Option<String>,
}

/// What the agent itself reports it would send at the start of a session in one directory
/// (Claude Code `/context`). Estimates by the agent, not counted from a log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextProbe {
    /// `empty directory` for the configuration alone, else the project name.
    pub place: String,
    pub dir: String,
    pub model: Option<String>,
    /// Tokens sent at session start: the sum of `sent`.
    pub total: u64,
    /// Categories sent with every request (system prompt, tools, memory files, skills, ...).
    pub sent: Vec<ProbeItem>,
    /// Categories listed by name only and loaded on demand; not in `total`.
    pub deferred: Vec<ProbeItem>,
    /// Memory files (CLAUDE.md and the like) with their paths.
    pub memory_files: Vec<ProbeItem>,
    /// MCP tool tokens summed per server.
    pub mcp_servers: Vec<ProbeItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeItem {
    pub label: String,
    pub tokens: u64,
}

/// The result of running the probes: what ran, and what could not.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Probe {
    /// The command run, for the report.
    pub command: String,
    pub runs: Vec<ContextProbe>,
    /// Place and reason of each probe that failed.
    pub failures: Vec<ProbeFailure>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeFailure {
    pub place: String,
    pub reason: String,
}

impl Probe {
    /// The run in an empty directory: the configuration without any project.
    pub fn baseline(&self) -> Option<&ContextProbe> {
        self.runs.iter().find(|r| r.place == PROBE_EMPTY_PLACE)
    }
}

/// [`ContextProbe::place`] of the run in an empty directory.
pub const PROBE_EMPTY_PLACE: &str = "empty directory";

/// Everything one adapter read.
#[derive(Debug, Default, Clone)]
pub struct Dataset {
    pub requests: Vec<Request>,
    pub compactions: Vec<Compaction>,
    /// Log files read, relative to the projects directory; `source` indexes into this.
    pub files: Vec<String>,
    pub session_starts: Vec<SessionStart>,
    pub stats: ReadStats,
}

/// Data quality of one source: what was observed and what could not be used.
#[derive(Debug, Serialize)]
pub struct DataQuality {
    /// Time of the first and last request in the logs (ISO 8601, UTC).
    pub observed_from: Option<String>,
    pub observed_to: Option<String>,
    pub requests: u64,
    /// False when there is no usage to analyse; then "no findings" would be misleading.
    pub analysable: bool,
    #[serde(flatten)]
    pub read: ReadStats,
}

impl ReadStats {
    /// Keeps the lowest and the highest version seen. Suffixes such as `-alpha.3` are ignored when
    /// comparing, `version` is kept as written.
    pub fn note_version(&mut self, version: &str) {
        let Some(key) = version_key(version) else {
            return;
        };
        if key == (0, 0, 0) {
            // Placeholder some logs carry (Codex rewrites it on resume); not a version.
            self.placeholder_version_entries += 1;
            return;
        }
        if self
            .first_version
            .as_deref()
            .and_then(version_key)
            .is_none_or(|c| key < c)
        {
            self.first_version = Some(version.to_owned());
        }
        if self
            .last_version
            .as_deref()
            .and_then(version_key)
            .is_none_or(|c| key > c)
        {
            self.last_version = Some(version.to_owned());
        }
    }
}

fn version_key(version: &str) -> Option<(u64, u64, u64)> {
    let numeric = version.split('-').next()?;
    let mut parts = numeric.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

impl Dataset {
    pub fn quality(&self) -> DataQuality {
        let first = self.requests.iter().map(|r| r.time_ms).min();
        let last = self.requests.iter().map(|r| r.time_ms).max();
        DataQuality {
            observed_from: first.map(crate::time::format_iso),
            observed_to: last.map(crate::time::format_iso),
            requests: self.requests.len() as u64,
            analysable: !self.requests.is_empty(),
            read: self.stats.clone(),
        }
    }

    /// Keeps only projects whose name contains `name`, ignoring case.
    pub fn retain_project(&mut self, name: &str) {
        let needle = name.to_lowercase();
        self.requests
            .retain(|r| r.project.to_lowercase().contains(&needle));
        self.compactions
            .retain(|c| c.project.to_lowercase().contains(&needle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_version_is_not_the_lowest_version() {
        let mut stats = ReadStats::default();
        for v in ["0.0.0", "0.130.0-alpha.5", "0.165.0", "0.0.0"] {
            stats.note_version(v);
        }
        assert_eq!(stats.first_version.as_deref(), Some("0.130.0-alpha.5"));
        assert_eq!(stats.last_version.as_deref(), Some("0.165.0"));
        assert_eq!(stats.placeholder_version_entries, 2);
    }
}
