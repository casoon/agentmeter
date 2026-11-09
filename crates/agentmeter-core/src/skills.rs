//! `--skills`: which installed skills were called in the period and how much of the session-start
//! skill list each one takes.
//!
//! Claude Code sends the name and description of every model-invocable skill with each session
//! start. The adapter finds the installed skills on disk (name and the length of the listing
//! entry, never the content); calls come from the `Skill` calls of the tool-call reader of #13
//! ([`crate::toolcalls::ToolEvent`] with the skill name in `target`). A skill without a call in the period is
//! "not observed in the period", never "never used": the log does not show a skill the user
//! starts by hand with `/name`, and a rarely used skill can still matter.
//!
//! Sizes are bytes of the listing entry (name plus description and `when_to_use`, cut at the
//! documented cap); tokens are estimated as bytes ÷ 4. The exact format of the list is not
//! documented, so both are estimates.

use crate::format::count;
use crate::instructions::estimate_tokens;
use crate::model::Probe;
use crate::time::format_iso;
use crate::toolcalls::ToolData;
use serde::Serialize;
use std::collections::HashMap;

/// Characters of `description` plus `when_to_use` that Claude Code keeps per listing entry.
pub const LISTING_CAP_CHARS: usize = 1_536;

pub const METHOD: &str = "Skill tool calls from the logs (opened only for the skill name); installed skills from \
     SKILL.md headers (name and description length only); entry size in bytes, tokens \
     estimated as bytes / 4, not tokenized";

/// The tool name of a skill call in Claude Code logs.
pub const SKILL_TOOL: &str = "Skill";

/// Whether a skill's listing entry is part of the session-start context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListState {
    /// Name and description are in the list sent at session start.
    Listed,
    /// `disable-model-invocation: true`: only the user can start it, nothing is listed.
    UserOnly,
    /// `SKILL.md` exists but could not be read.
    Unreadable,
}

#[derive(Debug, Clone)]
pub struct InstalledSkill {
    /// The name a call uses; plugin skills as `plugin:skill`.
    pub name: String,
    /// The directory name (with the plugin prefix), which also starts the skill.
    pub alias: Option<String>,
    /// `user`, `project` or `plugin`.
    pub source: &'static str,
    pub path: String,
    pub state: ListState,
    /// Bytes of the listing entry; 0 unless listed.
    pub bytes: u64,
}

/// What the adapter found on disk.
#[derive(Debug, Default, Clone)]
pub struct SkillScan {
    pub rules_version: &'static str,
    pub entries: Vec<InstalledSkill>,
    /// Limits of the scan: directories that are missing or unreadable, what is not covered.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillRow {
    pub name: String,
    /// `user`, `project` or `plugin`.
    pub source: &'static str,
    pub path: String,
    pub state: ListState,
    /// Calls in the selected period.
    pub calls: u64,
    /// Time of the latest call in all logs read (not limited by `--since`), or `None`.
    pub last_used: Option<String>,
    /// Bytes of the listing entry (estimate); 0 unless listed.
    pub list_bytes: u64,
    /// `list_bytes` as estimated tokens.
    pub list_est_tokens: u64,
}

/// A skill that was called but is not among the scanned ones (built in, synced from claude.ai,
/// nested or added directories, or removed since).
#[derive(Debug, Clone, Serialize)]
pub struct UnscannedCall {
    pub name: String,
    pub calls: u64,
    pub last_used: Option<String>,
}

/// The `Skills` category of the probe next to the scan.
#[derive(Debug, Clone, Serialize)]
pub struct SkillProbe {
    pub place: String,
    /// Tokens of the category `Skills` in `/context`, as Claude Code estimates them.
    pub probe_tokens: u64,
    /// Estimated tokens of the listed user and plugin skills found by the scan.
    pub scanned_est_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAnalysis {
    pub method: &'static str,
    pub rules_version: &'static str,
    /// `observed` (listed skills without a call), `checked_none` or `not_measurable`.
    pub status: &'static str,
    pub reason: Option<String>,
    pub message: Option<String>,
    /// Skill calls in the selected period.
    pub calls: u64,
    pub installed: u64,
    pub listed: u64,
    pub user_only: u64,
    pub unreadable: u64,
    /// Skills with at least one call in the period.
    pub used: u64,
    /// Listed skills without a call in the period.
    pub unused_listed: u64,
    pub listed_bytes: u64,
    pub listed_est_tokens: u64,
    pub unused_bytes: u64,
    pub unused_est_tokens: u64,
    pub largest_unused: Vec<SkillRow>,
    pub skills: Vec<SkillRow>,
    pub called_not_scanned: Vec<UnscannedCall>,
    pub probe: Option<SkillProbe>,
    pub next_step: Option<String>,
    pub notes: Vec<String>,
}

const MAX_LARGEST: usize = 5;

const NEXT_STEP: &str = "Check the largest unused entries: disable a plugin you do not use, \
     shorten a long description, or keep a skill that only one project needs in that \
     project's .claude/skills. A skill not observed in this period can still matter (rare or \
     seasonal use, or started by hand with /name, which the log does not show).";

/// Counts the calls of each scanned skill and totals the listing entries of those without one.
/// `unmeasurable` names the reason when the source logs no skill calls at all.
pub fn analyse(
    scan: &SkillScan,
    tools: Option<&ToolData>,
    since_ms: Option<i64>,
    probe: Option<&Probe>,
    unmeasurable: Option<&str>,
) -> SkillAnalysis {
    let mut notes = scan.notes.clone();
    notes.push(
        "Skills the user starts by hand with /name are not counted: the log records them only \
         in the prompt text, which is not read."
            .to_owned(),
    );
    let in_state =
        |state: ListState| scan.entries.iter().filter(|e| e.state == state).count() as u64;
    let listed_bytes: u64 = scan.entries.iter().map(|e| e.bytes).sum();
    let mut analysis = SkillAnalysis {
        method: METHOD,
        rules_version: scan.rules_version,
        status: "not_measurable",
        reason: None,
        message: None,
        calls: 0,
        installed: scan.entries.len() as u64,
        listed: in_state(ListState::Listed),
        user_only: in_state(ListState::UserOnly),
        unreadable: in_state(ListState::Unreadable),
        used: 0,
        unused_listed: 0,
        listed_bytes,
        listed_est_tokens: estimate_tokens(listed_bytes),
        unused_bytes: 0,
        unused_est_tokens: 0,
        largest_unused: Vec::new(),
        skills: Vec::new(),
        called_not_scanned: Vec::new(),
        probe: None,
        next_step: None,
        notes,
    };
    let probe_run = probe.and_then(Probe::baseline);
    if let Some(run) = probe_run
        && let Some(item) = run.sent.iter().find(|i| i.label == "Skills")
    {
        let scanned: u64 = scan
            .entries
            .iter()
            .filter(|e| e.source != "project")
            .map(|e| e.bytes)
            .sum();
        analysis.probe = Some(SkillProbe {
            place: run.place.clone(),
            probe_tokens: item.tokens,
            scanned_est_tokens: estimate_tokens(scanned),
        });
        analysis.notes.push(
            "The probe's Skills figure is Claude Code's own estimate and also covers built-in and \
             claude.ai skills the scan does not read; the two numbers are not expected to match."
                .to_owned(),
        );
    }
    if let Some(reason) = unmeasurable {
        analysis.reason = Some(reason.to_owned());
        return analysis;
    }
    let Some(tools) = tools else {
        analysis.reason = Some("tool calls were not read".to_owned());
        return analysis;
    };
    if tools
        .events
        .iter()
        .all(|e| since_ms.is_some_and(|s| e.time_ms < s))
    {
        analysis.reason = Some("no tool calls in the selected period".to_owned());
        return analysis;
    }

    // Calls by name: in the period, and the latest anywhere.
    let mut by_name: HashMap<&str, (u64, Option<i64>)> = HashMap::new();
    let mut period_calls = 0;
    for e in tools.events.iter().filter(|e| e.tool == SKILL_TOOL) {
        let Some(name) = e.target.as_deref() else {
            continue;
        };
        let entry = by_name.entry(name).or_default();
        entry.1 = entry.1.max(Some(e.time_ms));
        if since_ms.is_none_or(|s| e.time_ms >= s) {
            entry.0 += 1;
            period_calls += 1;
        }
    }
    analysis.calls = period_calls;

    // A call finds its skill by name, else by the directory name; the first of two skills
    // with one name wins.
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, e) in scan.entries.iter().enumerate() {
        index.entry(e.name.as_str()).or_insert(i);
    }
    for (i, e) in scan.entries.iter().enumerate() {
        if let Some(alias) = &e.alias {
            index.entry(alias.as_str()).or_insert(i);
        }
    }
    let mut calls = vec![(0u64, None::<i64>); scan.entries.len()];
    let mut unscanned = Vec::new();
    for (name, (n, last)) in &by_name {
        match index.get(name) {
            Some(&i) => {
                calls[i].0 += n;
                calls[i].1 = calls[i].1.max(*last);
            }
            None if *n > 0 => unscanned.push(UnscannedCall {
                name: (*name).to_owned(),
                calls: *n,
                last_used: last.map(format_iso),
            }),
            None => {}
        }
    }
    unscanned.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
    analysis.called_not_scanned = unscanned;

    let mut rows: Vec<SkillRow> = scan
        .entries
        .iter()
        .zip(&calls)
        .map(|(e, (n, last))| SkillRow {
            name: e.name.clone(),
            source: e.source,
            path: e.path.clone(),
            state: e.state,
            calls: *n,
            last_used: last.map(format_iso),
            list_bytes: e.bytes,
            list_est_tokens: estimate_tokens(e.bytes),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then(b.list_bytes.cmp(&a.list_bytes))
            .then(a.name.cmp(&b.name))
    });
    analysis.used = rows.iter().filter(|r| r.calls > 0).count() as u64;
    let mut unused: Vec<&SkillRow> = rows
        .iter()
        .filter(|r| r.state == ListState::Listed && r.calls == 0)
        .collect();
    analysis.unused_listed = unused.len() as u64;
    analysis.unused_bytes = unused.iter().map(|r| r.list_bytes).sum();
    analysis.unused_est_tokens = estimate_tokens(analysis.unused_bytes);
    unused.sort_by(|a, b| b.list_bytes.cmp(&a.list_bytes).then(a.name.cmp(&b.name)));
    analysis.largest_unused = unused.into_iter().take(MAX_LARGEST).cloned().collect();
    analysis.skills = rows;

    if analysis.installed == 0 {
        analysis.status = "checked_none";
        analysis.message = Some("no installed skills found in the directories scanned".to_owned());
    } else if analysis.unused_listed == 0 {
        analysis.status = "checked_none";
        analysis.message = Some(format!(
            "every one of the {} listed skills was called in the period",
            analysis.listed
        ));
    } else {
        analysis.status = "observed";
        analysis.message = Some(format!(
            "{} of {} listed skills were not observed in the period; their entries are about \
             {} bytes (≈ {} tokens, estimated) of the {} listed (≈ {} tokens)",
            count(analysis.unused_listed),
            count(analysis.listed),
            count(analysis.unused_bytes),
            count(analysis.unused_est_tokens),
            count(analysis.listed_bytes),
            count(analysis.listed_est_tokens)
        ));
        analysis.next_step = Some(NEXT_STEP.to_owned());
    }
    analysis
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolcalls::{ToolEvent, ToolKind, ToolStatus};

    fn skill(name: &str, alias: Option<&str>, source: &'static str, bytes: u64) -> InstalledSkill {
        InstalledSkill {
            name: name.to_owned(),
            alias: alias.map(str::to_owned),
            source,
            path: format!("/s/{name}/SKILL.md"),
            state: if bytes == 0 {
                ListState::UserOnly
            } else {
                ListState::Listed
            },
            bytes,
        }
    }

    fn call(seq: u64, time_ms: i64, tool: &str, target: Option<&str>) -> ToolEvent {
        ToolEvent {
            source: 0,
            session: "s".into(),
            project: "p".into(),
            sub: false,
            seq,
            time_ms,
            call_id: format!("c{seq}"),
            tool: tool.into(),
            kind: ToolKind::Other,
            target: target.map(str::to_owned),
            range: None,
            fingerprint: None,
            status: ToolStatus::Ok,
            output_bytes: None,
            has_result: true,
        }
    }

    fn scan() -> SkillScan {
        SkillScan {
            rules_version: "test",
            entries: vec![
                skill("a", None, "user", 400),
                skill("b", Some("b-dir"), "user", 800),
                skill("p:c", Some("p:c-dir"), "plugin", 1200),
                skill("manual", None, "user", 0),
            ],
            notes: Vec::new(),
        }
    }

    fn data(events: Vec<ToolEvent>) -> ToolData {
        ToolData {
            events,
            ..ToolData::default()
        }
    }

    #[test]
    fn counts_calls_by_name_or_directory_and_totals_the_unused_entries() {
        let t = data(vec![
            call(1, 1_000, SKILL_TOOL, Some("a")),
            call(2, 2_000, SKILL_TOOL, Some("a")),
            call(3, 3_000, SKILL_TOOL, Some("b-dir")),
            call(4, 4_000, SKILL_TOOL, Some("built-in")),
            call(5, 5_000, "Read", Some("a")),
        ]);
        let r = analyse(&scan(), Some(&t), None, None, None);
        assert_eq!(r.status, "observed");
        assert_eq!(r.calls, 4);
        let row = |n: &str| r.skills.iter().find(|s| s.name == n).unwrap();
        assert_eq!(row("a").calls, 2);
        assert_eq!(row("b").calls, 1);
        assert_eq!(row("p:c").calls, 0);
        assert_eq!((r.installed, r.listed, r.user_only, r.used), (4, 3, 1, 2));
        assert_eq!((r.unused_listed, r.unused_bytes), (1, 1200));
        assert_eq!(r.unused_est_tokens, 300);
        assert_eq!(r.largest_unused[0].name, "p:c");
        assert_eq!(r.called_not_scanned.len(), 1);
        assert_eq!(r.called_not_scanned[0].name, "built-in");
        assert!(r.next_step.is_some());
    }

    #[test]
    fn a_skill_called_before_the_period_is_not_observed_but_keeps_its_last_use() {
        let t = data(vec![
            call(1, 1_000, SKILL_TOOL, Some("a")),
            call(2, 9_000, "Read", None),
        ]);
        let r = analyse(&scan(), Some(&t), Some(5_000), None, None);
        let a = r.skills.iter().find(|s| s.name == "a").unwrap();
        assert_eq!(a.calls, 0);
        assert!(a.last_used.is_some());
        assert_eq!(r.unused_listed, 3);
        assert!(r.called_not_scanned.is_empty());
    }

    #[test]
    fn without_skill_calls_in_the_source_or_any_tool_calls_it_is_not_measurable() {
        let r = analyse(&scan(), None, None, None, Some("no skill calls logged"));
        assert_eq!(r.status, "not_measurable");
        assert_eq!(r.reason.as_deref(), Some("no skill calls logged"));
        assert!(r.skills.is_empty() && r.unused_listed == 0);
        assert_eq!(r.listed_bytes, 2400);
        let r = analyse(&scan(), Some(&data(vec![])), None, None, None);
        assert_eq!(r.status, "not_measurable");
    }

    #[test]
    fn all_used_or_nothing_installed_is_checked_none() {
        let t = data(vec![
            call(1, 1_000, SKILL_TOOL, Some("a")),
            call(2, 1_000, SKILL_TOOL, Some("b")),
            call(3, 1_000, SKILL_TOOL, Some("p:c")),
        ]);
        let r = analyse(&scan(), Some(&t), None, None, None);
        assert_eq!(r.status, "checked_none");
        assert!(r.next_step.is_none());
        let empty = SkillScan::default();
        let r = analyse(&empty, Some(&t), None, None, None);
        assert_eq!(r.status, "checked_none");
        assert_eq!(r.called_not_scanned.len(), 3);
    }
}
