//! Codex CLI adapter: reads `sessions/**/rollout-*.jsonl` and `archived_sessions/` of
//! `$CODEX_HOME` (default `~/.codex`) into a [`Dataset`].
//!
//! Usage comes from `event_msg` / `token_count` events, which carry a cumulative
//! `total_token_usage`. A request is the change of that total between two events; an event whose
//! total did not change (rate-limit updates) is ignored, and `last_token_usage` is never summed.
//! `input_tokens` includes `cached_input_tokens` and `output_tokens` includes reasoning (checked
//! on local rollouts: `total_tokens` is always `input + output`). Cache writes are only
//! recorded by newer versions; where they are absent the source is marked as not measured.
//!
//! The format is not documented. Reading is tolerant: unreadable lines, unknown entry types and
//! unknown usage fields are counted, never fatal; message content is never opened. Compressed
//! rollouts (`.jsonl.zst`) are streamed through ruzstd with a window and an output limit; damaged
//! ones are counted, never fatal.

mod raw;

use agentmeter_core::model::{Compaction, Dataset, ReadStats, Request, SessionStart};
use agentmeter_core::time::parse_iso_ms;
use raw::{RawCompacted, RawEvent, RawLine, RawMeta, RawTokens, RawTurnContext};
use ruzstd::decoding::{FrameDecoder, StreamingDecoder};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

/// Entry types that carry no usage and are expected in rollouts; anything else is counted as
/// unknown so format drift shows up in the report.
const KNOWN_TYPES: &[&str] = &[
    "session_meta",
    "event_msg",
    "response_item",
    "turn_context",
    "compacted",
    "token_usage_record",
    "world_state",
    "inter_agent_communication_metadata",
    "realtime_item",
];

/// Usage fields that are known but not needed: reasoning is part of `output_tokens`, and
/// `total_tokens` is `input + output`.
const KNOWN_USAGE_FIELDS: &[&str] = &["reasoning_output_tokens", "total_tokens"];

/// Largest window a compressed rollout may declare; a larger one is refused before anything is
/// allocated (ruzstd's own default, stated here so the bound is visible).
const MAX_WINDOW_BYTES: u64 = 128 * 1024 * 1024;
/// Most bytes read from one compressed rollout; a defective or hostile file cannot grow further.
const MAX_DECOMPRESSED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Streams the frames of a `.zst` file one after the other, like `zstd -dc`, without holding the
/// decompressed data in memory.
struct ZstdFrames<R: Read> {
    frame: Option<StreamingDecoder<R, FrameDecoder>>,
}

impl<R: BufRead> ZstdFrames<R> {
    fn new(source: R) -> io::Result<Self> {
        Ok(ZstdFrames {
            frame: Some(open_frame(source)?),
        })
    }
}

fn open_frame<R: Read>(source: R) -> io::Result<StreamingDecoder<R, FrameDecoder>> {
    StreamingDecoder::new_with_max_window_size(source, MAX_WINDOW_BYTES).map_err(io::Error::other)
}

impl<R: BufRead> Read for ZstdFrames<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while let Some(frame) = self.frame.as_mut() {
            let n = frame.read(out)?;
            if n > 0 || out.is_empty() {
                return Ok(n);
            }
            // The frame ended; another one may follow.
            let mut source = self
                .frame
                .take()
                .map(StreamingDecoder::into_inner)
                .expect("frame");
            if source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.frame = Some(open_frame(source)?);
        }
        Ok(0)
    }
}

/// Directories inside the Codex home that hold rollouts.
const ROLLOUT_DIRS: &[&str] = &["sessions", "archived_sessions"];

/// `$CODEX_HOME` if set, else `~/.codex`.
pub fn config_dirs() -> Vec<PathBuf> {
    if let Some(value) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(value)];
    }
    std::env::var_os("HOME")
        .map(|home| vec![PathBuf::from(home).join(".codex")])
        .unwrap_or_default()
}

/// Reads the rollout directories of every Codex home. Missing directories are skipped; the
/// caller can tell from `stats.files` and `stats.compressed_files_unreadable`.
pub fn read(homes: &[PathBuf]) -> Dataset {
    let mut reader = Reader::default();
    for home in homes {
        let mut files = Vec::new();
        for name in ROLLOUT_DIRS {
            collect_files(&home.join(name), &mut files);
        }
        // A rollout that exists in plain text and compressed is read once: the plain one wins.
        let plain: HashSet<OsString> = files
            .iter()
            .filter(|f| !is_compressed(f))
            .filter_map(|f| f.file_name().map(OsStr::to_os_string))
            .collect();
        for file in files {
            if is_compressed(&file) && plain.contains(&uncompressed_name(&file)) {
                reader.stats.compressed_files_shadowed += 1;
            } else {
                reader.read_file(home, &file);
            }
        }
    }
    reader.finish()
}

fn is_compressed(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("zst"))
}

/// `rollout-x.jsonl.zst` -> `rollout-x.jsonl`.
fn uncompressed_name(path: &Path) -> OsString {
    path.file_stem()
        .map(OsStr::to_os_string)
        .unwrap_or_default()
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
            Ok(kind) if kind.is_file() => {
                let name = path.to_string_lossy();
                if name.ends_with(".jsonl.zst") || path.extension() == Some(OsStr::new("jsonl")) {
                    out.push(path);
                }
            }
            _ => {}
        }
    }
}

/// Cumulative token counters of one thread.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct Totals {
    input: u64,
    cached: u64,
    write: u64,
    output: u64,
}

impl Totals {
    fn of(tokens: &RawTokens) -> Totals {
        Totals {
            input: tokens.input_tokens.unwrap_or(0),
            cached: tokens.cached_input_tokens.unwrap_or(0),
            write: tokens.cache_write_input_tokens.unwrap_or(0),
            output: tokens.output_tokens.unwrap_or(0),
        }
    }

    /// `self - earlier`, or `None` if any counter went backwards.
    fn since(self, earlier: Totals) -> Option<Totals> {
        Some(Totals {
            input: self.input.checked_sub(earlier.input)?,
            cached: self.cached.checked_sub(earlier.cached)?,
            write: self.write.checked_sub(earlier.write)?,
            output: self.output.checked_sub(earlier.output)?,
        })
    }
}

/// What one file showed.
#[derive(Default)]
struct FileFacts {
    usage_events: u64,
    /// A first event with a changed total was counted.
    started: bool,
    session: String,
    project: String,
    sub: bool,
    metas: u64,
    foreign_meta: bool,
    forked: bool,
    first_is_meta: Option<bool>,
    /// `originator` and `cwd` of the first `session_meta`.
    entrypoint: Option<String>,
    cwd: Option<String>,
    model: String,
    previous: Totals,
    last_context: u64,
}

#[derive(Default)]
struct Reader {
    stats: ReadStats,
    requests: Vec<Request>,
    compactions: Vec<Compaction>,
    files: Vec<String>,
    session_starts: Vec<SessionStart>,
    /// Events already counted from another file (a sub-agent rollout replays its parent).
    seen_events: HashSet<(String, u64, u64, u64)>,
    seen_compactions: HashSet<String>,
}

impl Reader {
    fn read_file(&mut self, home: &Path, path: &Path) {
        let compressed = is_compressed(path);
        let Ok(file) = File::open(path) else {
            return;
        };
        let input: Box<dyn Read> = if compressed {
            match ZstdFrames::new(BufReader::new(file)) {
                Ok(frames) => Box::new(frames.take(MAX_DECOMPRESSED_BYTES)),
                Err(_) => {
                    self.stats.compressed_files_unreadable += 1;
                    return;
                }
            }
        } else {
            Box::new(file)
        };
        self.stats.files += 1;
        self.stats.compressed_files_read += u64::from(compressed);
        let source = self.files.len();
        let relative = path.strip_prefix(home).unwrap_or(path);
        self.files.push(relative.to_string_lossy().into_owned());
        let name = if compressed {
            uncompressed_name(path)
        } else {
            path.file_name()
                .map(OsStr::to_os_string)
                .unwrap_or_default()
        };
        let file_stem = Path::new(&name)
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());

        let mut facts = FileFacts {
            session: file_stem.clone(),
            project: "unknown".to_owned(),
            ..FileFacts::default()
        };
        let mut reader = BufReader::new(input);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => {
                    // A damaged or oversized compressed stream: keep what was read, say so.
                    self.stats.compressed_files_unreadable += u64::from(compressed);
                    break;
                }
            }
            if buffer.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            self.stats.lines += 1;
            let Ok(line) = serde_json::from_slice::<RawLine>(&buffer) else {
                self.stats.skipped_lines += 1;
                continue;
            };
            if facts.first_is_meta.is_none() {
                facts.first_is_meta = Some(line.kind.as_deref() == Some("session_meta"));
            }
            match line.kind.as_deref() {
                Some("session_meta") => self.session_meta(&line, &mut facts),
                Some("turn_context") => {
                    if let Some(context) = payload::<RawTurnContext>(&line) {
                        facts.model = context.model.unwrap_or_default();
                    }
                }
                Some("event_msg") => {
                    if let Some(event) = payload::<RawEvent>(&line)
                        && event.kind.as_deref() == Some("token_count")
                        && let Some(info) = event.info
                    {
                        self.token_count(&line, info, source, &mut facts);
                    }
                }
                Some("compacted") => self.compacted(&line, source, &facts),
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
        self.stats.files_without_usage += u64::from(facts.usage_events == 0);
        if !facts.sub {
            let reason = if facts.foreign_meta || facts.forked {
                Some("inherited-history")
            } else if facts.first_is_meta != Some(true) {
                Some("no-log-start")
            } else {
                None
            };
            self.session_starts.push(SessionStart {
                session: facts.session,
                source,
                clean: reason.is_none(),
                reason,
                entrypoint: facts.entrypoint.take(),
                cwd: facts.cwd.take(),
            });
        }
    }

    fn session_meta(&mut self, line: &RawLine, facts: &mut FileFacts) {
        let Some(meta) = payload::<RawMeta>(line) else {
            self.stats.skipped_lines += 1;
            return;
        };
        facts.metas += 1;
        if facts.metas == 1 {
            if let Some(id) = meta.id.filter(|id| !id.is_empty()) {
                facts.session = id;
            }
            facts.sub = meta.thread_source.as_deref() == Some("subagent");
            facts.forked = meta.forked_from_id.is_some();
            if let Some(name) = meta.cwd.as_deref().and_then(project_name) {
                facts.project = name;
            }
            facts.entrypoint = meta.originator.filter(|o| !o.is_empty());
            facts.cwd = meta.cwd.filter(|c| !c.is_empty());
            // Shown only; Resume rewrites it, so it never selects behaviour.
            if let Some(version) = meta.cli_version.as_deref() {
                self.stats.note_version(version);
            }
        } else if meta.id.as_deref().is_some_and(|id| id != facts.session) {
            // A later meta of another thread is the replayed history of a parent.
            facts.foreign_meta = true;
        }
    }

    fn token_count(
        &mut self,
        line: &RawLine,
        info: raw::RawInfo,
        source: usize,
        facts: &mut FileFacts,
    ) {
        let Some(total_raw) = info.total_token_usage else {
            self.stats.skipped_lines += 1;
            return;
        };
        let Some(time_ms) = line.timestamp.as_deref().and_then(parse_iso_ms) else {
            self.stats.skipped_lines += 1;
            return;
        };
        facts.usage_events += 1;
        for raw in [Some(&total_raw), info.last_token_usage.as_ref()]
            .into_iter()
            .flatten()
        {
            for field in raw
                .rest
                .keys()
                .filter(|f| !KNOWN_USAGE_FIELDS.contains(&f.as_str()))
            {
                *self
                    .stats
                    .unknown_usage_fields
                    .entry(field.clone())
                    .or_default() += 1;
            }
        }
        let total = Totals::of(&total_raw);
        if total == facts.previous {
            // Rate-limit updates repeat the unchanged total.
            self.stats.duplicates_merged += 1;
            return;
        }
        let key = (
            line.timestamp.clone().unwrap_or_default(),
            total.input,
            total.output,
            total.cached,
        );
        if !self.seen_events.insert(key) {
            self.stats.replays_dropped += 1;
            return;
        }
        let last = info.last_token_usage.as_ref().map(Totals::of);
        // A forked rollout starts with the running total of its parent: the first event then
        // carries a total far above its own request, and only `last_token_usage` is this thread's.
        let inherited = !facts.started
            && last.is_some_and(|l| (l.input, l.output) != (total.input, total.output));
        facts.started = true;
        let delta = match total.since(facts.previous) {
            _ if inherited => {
                self.stats.usage_delta_mismatches += 1;
                last.unwrap_or(total)
            }
            Some(delta) => {
                if last.is_some_and(|l| (l.input, l.output) != (delta.input, delta.output)) {
                    self.stats.usage_delta_mismatches += 1;
                }
                delta
            }
            None => {
                // The total went backwards: a new counter, so the event's own value stands.
                self.stats.usage_delta_mismatches += 1;
                last.unwrap_or(total)
            }
        };
        facts.previous = total;
        if delta.input == 0 && delta.output == 0 {
            return;
        }
        self.stats.cache_write_measured |= delta.write > 0;
        facts.last_context = delta.input;
        self.requests.push(Request {
            session: facts.session.clone(),
            project: facts.project.clone(),
            source,
            message_id: line
                .ordinal
                .map_or_else(String::new, |n| format!("ordinal:{n}")),
            time_ms,
            model: facts.model.clone(),
            sub: facts.sub,
            fresh: delta
                .input
                .saturating_sub(delta.cached)
                .saturating_sub(delta.write),
            cache_read: delta.cached,
            cache_write_5m: delta.write,
            cache_write_1h: 0,
            output: delta.output,
            window: info.model_context_window,
        });
    }

    fn compacted(&mut self, line: &RawLine, source: usize, facts: &FileFacts) {
        let Some(time_ms) = line.timestamp.as_deref().and_then(parse_iso_ms) else {
            self.stats.skipped_lines += 1;
            return;
        };
        let id = payload::<RawCompacted>(line)
            .and_then(|c| c.compaction_response_id)
            .filter(|id| !id.is_empty());
        if let Some(id) = &id
            && !self.seen_compactions.insert(id.clone())
        {
            return;
        }
        self.stats.compactions_without_post_tokens += 1;
        self.compactions.push(Compaction {
            session: facts.session.clone(),
            project: facts.project.clone(),
            source,
            event_id: id.unwrap_or_else(|| {
                line.ordinal
                    .map_or_else(String::new, |n| format!("ordinal:{n}"))
            }),
            time_ms,
            // The context of the last request before it; the log has no size of its own.
            tokens_before: facts.last_context,
            tokens_after: None,
            auto: None,
        });
    }

    fn finish(mut self) -> Dataset {
        self.requests.sort_by(|a, b| {
            (a.time_ms, &a.session, a.source, a.output)
                .cmp(&(b.time_ms, &b.session, b.source, b.output))
        });
        self.compactions
            .sort_by(|a, b| (a.time_ms, &a.session).cmp(&(b.time_ms, &b.session)));
        self.stats.requests_without_model =
            self.requests.iter().filter(|r| r.model.is_empty()).count() as u64;
        Dataset {
            requests: self.requests,
            compactions: self.compactions,
            files: self.files,
            session_starts: self.session_starts,
            stats: self.stats,
        }
    }
}

fn payload<'a, T: serde::Deserialize<'a>>(line: &'a RawLine) -> Option<T> {
    line.payload
        .and_then(|raw| serde_json::from_str(raw.get()).ok())
}

/// Last path component of the working directory, the project name shown in reports.
fn project_name(cwd: &str) -> Option<String> {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}
