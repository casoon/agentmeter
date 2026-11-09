//! Instruction files of one project (`--instructions`): which files an agent is expected to
//! load, how large they are and what that suggests.
//!
//! The adapters find the files by the loading rules of their agent; this module holds the shared
//! result and the rules that need no file access. Today's files only, never the history.

use serde::Serialize;
use std::collections::HashMap;

/// Estimated tokens per byte of text: bytes ÷ 4. A rule of thumb, not a tokenizer.
pub fn estimate_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

/// Estimated tokens in the instruction files expected at session start from which `base-size`
/// reports. Uncalibrated, like the thresholds of the log rules.
pub const BASE_TOKENS: u64 = 10_000;

/// How sure we are that a file is part of the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadState {
    /// Listed by an observation: the agent's own `/context` (`--probe`) names the file.
    Confirmed,
    /// The loading rules say it is loaded at session start; not observed.
    Expected,
    /// Loaded only when the agent touches a matching file or directory; not at session start.
    OnDemand,
    /// Exists but the rules say it is not loaded (shadowed, excluded, beyond a limit).
    NotLoaded,
    /// Exists but could not be read.
    Unreadable,
    /// Referenced by another file but not there.
    Missing,
}

impl LoadState {
    /// Counts towards the context at session start.
    pub fn is_loaded(self) -> bool {
        matches!(self, LoadState::Confirmed | LoadState::Expected)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InstructionFile {
    pub path: String,
    /// `managed`, `user`, `ancestor`, `project`, `rules`, `nested`, `import`.
    pub scope: &'static str,
    pub state: LoadState,
    /// Size of the file on disk; 0 for unreadable or missing files.
    pub bytes: u64,
    /// `bytes` as estimated tokens (bytes ÷ 4).
    pub est_tokens: u64,
    /// Bytes expected in the context at session start: 0 unless the state is loaded; less than
    /// `bytes` when a limit cuts the file.
    pub loaded_bytes: u64,
    /// The file that imports this one.
    pub via: Option<String>,
    /// Why this state: the rule or the problem.
    pub note: Option<String>,
    #[serde(skip)]
    pub fingerprint: Option<u64>,
}

impl InstructionFile {
    pub fn new(path: String, scope: &'static str, state: LoadState) -> InstructionFile {
        InstructionFile {
            path,
            scope,
            state,
            bytes: 0,
            est_tokens: 0,
            loaded_bytes: 0,
            via: None,
            note: None,
            fingerprint: None,
        }
    }

    /// Sets the size from the content and, for a loaded state, the loaded bytes.
    pub fn with_content(mut self, content: &[u8]) -> InstructionFile {
        use std::hash::{DefaultHasher, Hash, Hasher};
        self.bytes = content.len() as u64;
        self.est_tokens = estimate_tokens(self.bytes);
        if self.state.is_loaded() {
            self.loaded_bytes = self.bytes;
        }
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        self.fingerprint = Some(hasher.finish());
        self
    }
}

/// What a rule found in the files; same idea as a log finding: rule, measure, certainty and a
/// concrete step. Never an error.
#[derive(Debug, Clone, Serialize)]
pub struct InstructionFinding {
    pub rule: &'static str,
    pub file: String,
    pub measure: String,
    pub certainty: &'static str,
    pub check: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstructionScan {
    pub tool: &'static str,
    /// The loading rules the scan follows, with the documentation they come from.
    pub rules_version: &'static str,
    /// How sizes were measured.
    pub method: &'static str,
    pub dir: String,
    pub files: Vec<InstructionFile>,
    /// Estimated tokens of the files expected (or confirmed) at session start.
    pub loaded_est_tokens: u64,
    pub findings: Vec<InstructionFinding>,
    /// Limits of the scan that the reader should know.
    pub notes: Vec<String>,
}

pub const METHOD: &str = "file size in bytes; tokens estimated as bytes / 4, not tokenized";

impl InstructionScan {
    pub fn new(
        tool: &'static str,
        rules_version: &'static str,
        dir: String,
        files: Vec<InstructionFile>,
        notes: Vec<String>,
    ) -> InstructionScan {
        InstructionScan {
            tool,
            rules_version,
            method: METHOD,
            dir,
            files,
            loaded_est_tokens: 0,
            findings: Vec::new(),
            notes,
        }
    }

    /// Adds the totals and the findings that follow from the files alone.
    pub fn finish(mut self) -> InstructionScan {
        self.loaded_est_tokens = estimate_tokens(self.files.iter().map(|f| f.loaded_bytes).sum());
        let mut findings = Vec::new();
        for file in self.files.iter().filter(|f| f.state == LoadState::Missing) {
            findings.push(InstructionFinding {
                rule: "broken-import",
                file: file.path.clone(),
                measure: format!(
                    "imported by {}, not found",
                    file.via.as_deref().unwrap_or("?")
                ),
                certainty: "medium",
                check: "Fix the path or remove the line; if the text is no import, ignore this."
                    .to_owned(),
            });
        }
        let mut groups: HashMap<(u64, u64), Vec<&InstructionFile>> = HashMap::new();
        for file in &self.files {
            if let (Some(fp), true) = (file.fingerprint, file.bytes > 0)
                && (file.state.is_loaded() || file.state == LoadState::OnDemand)
            {
                groups.entry((fp, file.bytes)).or_default().push(file);
            }
        }
        let mut duplicates: Vec<Vec<&InstructionFile>> = groups
            .into_values()
            .filter(|g| g.iter().filter(|f| f.state != LoadState::OnDemand).count() >= 2)
            .collect();
        duplicates.sort_by(|a, b| a[0].path.cmp(&b[0].path));
        for group in duplicates {
            let paths: Vec<&str> = group.iter().map(|f| f.path.as_str()).collect();
            findings.push(InstructionFinding {
                rule: "duplicate-instructions",
                file: paths[0].to_owned(),
                measure: format!(
                    "{} files with identical content, {} bytes each (≈ {} tokens each): {}",
                    group.len(),
                    group[0].bytes,
                    group[0].est_tokens,
                    paths.join(", ")
                ),
                certainty: "high",
                check: "Keep one copy and import it from the others, or delete the extra file."
                    .to_owned(),
            });
        }
        if self.loaded_est_tokens >= BASE_TOKENS {
            findings.push(InstructionFinding {
                rule: "base-size",
                file: self.dir.clone(),
                measure: format!(
                    "≈ {} tokens (estimated) expected at session start, from {} files; hint from {}",
                    self.loaded_est_tokens,
                    self.files.iter().filter(|f| f.state.is_loaded()).count(),
                    BASE_TOKENS
                ),
                certainty: "low",
                check: "Large is not wrong. See which files are the biggest and whether their \
                        content is needed in every session; move rarely needed parts to \
                        on-demand rules or skills."
                    .to_owned(),
            });
        }
        findings.append(&mut self.findings);
        self.findings = findings;
        self
    }
}
