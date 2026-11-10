//! `agentmeter` — diagnosis for coding-agent usage.
//!
//! Reads the local Claude Code and Codex CLI session logs and reports totals and findings.
//! Read-only.

mod captures;
mod cli;
mod menu;
mod render;

use agentmeter_core::instructions::{InstructionScan, LoadState};
use agentmeter_core::model::{Dataset, PROBE_EMPTY_PLACE, Probe};
use agentmeter_core::report::{ProjectRow, SessionRow, Summary, projects, sessions, summarize};
use agentmeter_core::time::format_day;
use agentmeter_core::toolcalls::{ToolAnalysis, ToolData};
use agentmeter_core::{Compaction, DataQuality, Evaluation, Request, check, evaluate};
use cli::{Check, Invocation, Options, Since, Tool};
use render::{Report, Style};
use runemark::{ColorMode, Console, ProgressMode, ProgressSink, TerminalProgress, Verdict};
use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The menu needs a person: no arguments, and both streams are terminals (the menu draws on
    // stderr, the list it replaces goes to stdout).
    let interactive = args.is_empty() && io::stdout().is_terminal() && io::stderr().is_terminal();
    match cli::parse(args) {
        Invocation::Help => {
            println!("{}", cli::help(VERSION));
            ExitCode::SUCCESS
        }
        Invocation::Version => {
            println!("agentmeter {VERSION}");
            ExitCode::SUCCESS
        }
        Invocation::Error(message) => {
            eprintln!("agentmeter: {message}\nTry `agentmeter --help`.");
            ExitCode::from(2)
        }
        Invocation::Run(options) => run(&options, interactive),
    }
}

/// One agent's data, read and not yet analysed.
pub struct Source {
    pub tool: &'static str,
    pub label: &'static str,
    places: Vec<PathBuf>,
    pub dataset: Dataset,
    /// Tool calls; only read with `--tool-calls` or `--skills`.
    pub tools: Option<ToolData>,
}

/// The result of analysing one source; borrowed by [`Report`].
pub struct Analysis {
    pub quality: DataQuality,
    /// Requests the selected period holds.
    pub requests_in_period: u64,
    pub summary: Option<Summary>,
    pub sessions: Option<Vec<SessionRow>>,
    pub projects: Option<Vec<ProjectRow>>,
    pub findings: Option<Evaluation>,
    /// What `--probe` asked Claude Code; only for the Claude source.
    pub probe: Option<Probe>,
    /// What `--instructions` found for this source.
    pub instructions: Option<InstructionScan>,
    /// What `--tool-calls` found.
    pub tool_calls: Option<ToolAnalysis>,
    /// What `--transitions` found.
    pub transitions: Option<agentmeter_core::transitions::TransitionReport>,
    /// What `--compare` found.
    pub compare: Option<agentmeter_core::compare::Comparison>,
    /// What `--subagents` found.
    pub subagents: Option<agentmeter_core::subagents::SubagentReport>,
    /// What `--skills` found.
    pub skills: Option<agentmeter_core::skills::SkillAnalysis>,
    /// What `--compaction-causes` found.
    pub compaction_causes: Option<agentmeter_core::compaction::CausesReport>,
    /// What `--changed-on` found.
    pub before_after: Option<agentmeter_core::before_after::BeforeAfter>,
    /// What `--retention` found.
    pub retention: Option<agentmeter_core::gaps::Retention>,
    /// What `--ai-report` condensed from the analyses above.
    pub ai_report: Option<agentmeter_core::ai_report::AiReport>,
}

/// Runs `read` and shows on stderr how far it is once it has taken a second, since large logs
/// take a while. Only on a terminal: a redirected stderr stays as it was.
fn with_progress<T>(
    label: &str,
    tool_calls: bool,
    read: impl FnOnce(&mut dyn FnMut(u64, u64)) -> T,
) -> T {
    let terminal = io::stderr().is_terminal();
    let mode = if terminal {
        ProgressMode::Auto
    } else {
        ProgressMode::Never
    };
    let sink = TerminalProgress::stderr(mode, Console::stderr(ColorMode::Auto), terminal);
    let what = if tool_calls {
        format!("{label} logs with tool calls")
    } else {
        format!("{label} logs")
    };
    let begun = Instant::now();
    let mut shown = None;
    let out = read(&mut |done, total| {
        if shown.is_none() && begun.elapsed() >= Duration::from_secs(1) {
            sink.start(total, &what);
            shown = Some(total);
        }
        if shown.is_some() {
            sink.advance(done, &what);
        }
    });
    if let Some(files) = shown {
        sink.finish(Verdict::Passed, &format!("{what}: {files} files read"));
    }
    out
}

pub fn read_sources(tool: Tool) -> Vec<Source> {
    read_sources_with(tool, false)
}

pub fn read_sources_with(tool: Tool, tool_calls: bool) -> Vec<Source> {
    let mut sources = Vec::new();
    if matches!(tool, Tool::All | Tool::Claude) {
        let dirs = agentmeter_claude::config_dirs();
        let places = dirs.iter().map(|d| d.join("projects")).collect();
        let (dataset, tools) = with_progress("Claude Code", tool_calls, |progress| {
            agentmeter_claude::read_with_progress(&dirs, tool_calls, progress)
        });
        sources.push(Source {
            tool: "claude",
            label: "Claude Code",
            places,
            dataset,
            tools: tool_calls.then_some(tools),
        });
    }
    if matches!(tool, Tool::All | Tool::Codex) {
        let homes = agentmeter_codex::config_dirs();
        let places = homes.iter().map(|h| h.join("sessions")).collect();
        let (dataset, tools) = with_progress("Codex CLI", tool_calls, |progress| {
            agentmeter_codex::read_with_progress(&homes, tool_calls, progress)
        });
        sources.push(Source {
            tool: "codex",
            label: "Codex CLI",
            places,
            dataset,
            tools: tool_calls.then_some(tools),
        });
    }
    // Without an explicit choice, a source that has no logs at all is left out.
    if tool == Tool::All {
        sources.retain(|s| {
            s.dataset.stats.files > 0 || s.dataset.stats.compressed_files_unreadable > 0
        });
    }
    sources
}

pub fn analyse(source: &Source, options: &Options, now_ms: i64, probe: Option<Probe>) -> Analysis {
    let dataset = &source.dataset;
    let since_ms = options.since.map(|s| s.start_ms(now_ms));
    let in_window = |time_ms: i64| since_ms.is_none_or(|since| time_ms >= since);
    let requests: Vec<&Request> = dataset
        .requests
        .iter()
        .filter(|r| in_window(r.time_ms))
        .collect();
    let compactions: Vec<&Compaction> = dataset
        .compactions
        .iter()
        .filter(|c| in_window(c.time_ms))
        .collect();
    Analysis {
        quality: dataset.quality(),
        requests_in_period: requests.len() as u64,
        summary: options.summary.then(|| summarize(&requests, &compactions)),
        sessions: options.sessions.then(|| sessions(&requests, &compactions)),
        projects: options.projects.then(|| projects(&requests)),
        findings: options
            .findings
            .then(|| evaluate(dataset, since_ms, probe.as_ref())),
        probe,
        instructions: None,
        ai_report: None,
        compare: options.compare.map(|days| {
            let (earlier, later) = agentmeter_core::compare::windows_before(now_ms, days);
            agentmeter_core::compare::compare(dataset, earlier, later)
        }),
        retention: options.retention.then(|| {
            // The settings files are read only when asked for, and only for Claude Code.
            if source.tool == "claude" {
                agentmeter_claude::settings::retention(
                    &agentmeter_claude::config_dirs(),
                    dataset,
                    now_ms,
                )
            } else {
                agentmeter_core::gaps::not_measurable(CODEX_RETENTION_REASON)
            }
        }),
        before_after: options.changed_on.map(|change_ms| {
            agentmeter_core::before_after::before_after(
                dataset,
                change_ms,
                options.change_label.as_deref(),
                now_ms,
            )
        }),
        transitions: options.transitions.then(|| {
            // Tool calls read only for `--skills` do not feed the timeline.
            let tools = source.tools.as_ref().filter(|_| options.tool_calls);
            agentmeter_core::transitions::analyse(dataset, tools, since_ms)
        }),
        subagents: options.subagents.then(|| {
            // Tool calls read only for `--skills` do not feed the overlap check.
            let tools = source.tools.as_ref().filter(|_| options.tool_calls);
            agentmeter_core::subagents::analyse(dataset, tools, since_ms)
        }),
        skills: None,
        compaction_causes: options.compaction_causes.then(|| {
            // The settings files are read only when asked for, and only for Claude Code.
            let settings = (options.compaction_settings && source.tool == "claude").then(|| {
                agentmeter_claude::settings::read(&agentmeter_claude::config_dirs(), dataset)
            });
            agentmeter_core::compaction::analyse(
                dataset,
                since_ms,
                settings,
                options.compaction_settings,
            )
        }),
        tool_calls: source
            .tools
            .as_ref()
            .filter(|_| options.tool_calls)
            .map(|t| {
                agentmeter_core::toolcalls::evaluate(
                    t,
                    &dataset.files,
                    &dataset.compactions,
                    since_ms,
                )
            }),
    }
}

/// Why Codex has no retention to read.
const CODEX_RETENTION_REASON: &str = "not measurable: Codex CLI has no setting that deletes its \
     session files by age (config.toml only caps history.jsonl with history.max_bytes, which is not \
     the session log); nothing is read";

/// Place name of the directory given to `--instructions` in a probe.
pub const PROBE_INSTRUCTIONS_PLACE: &str = "instructions directory";

/// Projects probed besides the empty directory: the busiest by requests in the period.
const PROBE_PROJECTS: usize = 3;

/// Where to probe: an empty directory for the configuration alone, and the working directory
/// of the latest session of each of the busiest projects, if it still exists.
pub fn probe_places(
    dataset: &Dataset,
    since_ms: Option<i64>,
    empty: &Path,
    extra: Option<&Path>,
) -> Vec<(String, PathBuf)> {
    let in_window = |time_ms: i64| since_ms.is_none_or(|since| time_ms >= since);
    let mut requests: HashMap<&str, u64> = HashMap::new();
    // session -> (project, first request time)
    let mut sessions: HashMap<&str, (&str, i64)> = HashMap::new();
    for r in dataset.requests.iter().filter(|r| in_window(r.time_ms)) {
        *requests.entry(r.project.as_str()).or_default() += 1;
        let entry = sessions
            .entry(r.session.as_str())
            .or_insert((r.project.as_str(), r.time_ms));
        entry.1 = entry.1.min(r.time_ms);
    }
    let mut ranked: Vec<(&str, u64)> = requests.into_iter().collect();
    ranked.sort_by_key(|(project, n)| (std::cmp::Reverse(*n), *project));
    let mut places = vec![(PROBE_EMPTY_PLACE.to_owned(), empty.to_path_buf())];
    for (project, _) in ranked {
        if places.len() > PROBE_PROJECTS {
            break;
        }
        let latest = dataset
            .session_starts
            .iter()
            .filter_map(|s| {
                let (p, time) = sessions.get(s.session.as_str())?;
                let dir = PathBuf::from(s.cwd.as_deref()?);
                (*p == project && dir.is_dir()).then_some((*time, dir))
            })
            .max_by_key(|(time, _)| *time);
        if let Some((_, dir)) = latest {
            places.push((project.to_owned(), dir));
        }
    }
    if let Some(dir) = extra {
        places.push((PROBE_INSTRUCTIONS_PLACE.to_owned(), dir.to_path_buf()));
    }
    places
}

/// Runs the probe for the Claude source and says on stderr what it does, since it takes seconds.
pub fn run_probe(
    source: &Source,
    since_ms: Option<i64>,
    tell: bool,
    extra: Option<&Path>,
) -> Probe {
    let empty = match agentmeter_claude::probe::EmptyDir::create() {
        Ok(dir) => dir,
        Err(e) => {
            return Probe {
                failures: vec![agentmeter_core::model::ProbeFailure {
                    place: PROBE_EMPTY_PLACE.to_owned(),
                    reason: format!("could not create a temporary directory: {e}"),
                }],
                ..Probe::default()
            };
        }
    };
    let places = probe_places(&source.dataset, since_ms, &empty.0, extra);
    if tell {
        let names: Vec<&str> = places.iter().map(|(p, _)| p.as_str()).collect();
        eprintln!(
            "agentmeter: asking Claude Code for /context in {} ({})…",
            if places.len() == 1 {
                "1 place".to_owned()
            } else {
                format!("{} places", places.len())
            },
            names.join(", ")
        );
    }
    agentmeter_claude::probe::run(&places)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn run(options: &Options, interactive: bool) -> ExitCode {
    if let Some(code) = captures::store_action(options) {
        return code;
    }
    let instructions_dir = match options.instructions.as_deref().map(Path::new) {
        Some(dir) if !dir.is_dir() => {
            eprintln!(
                "agentmeter: --instructions: `{}` is not a directory",
                dir.display()
            );
            return ExitCode::from(2);
        }
        other => other,
    };
    let with_tools = options.tool_calls || options.skills;
    let mut sources = read_sources_with(options.tool, with_tools);
    // A file scan needs no logs: the sources are the agents asked for.
    if instructions_dir.is_some() && options.tool == Tool::All {
        sources = read_sources_with(Tool::Claude, with_tools)
            .into_iter()
            .chain(read_sources_with(Tool::Codex, with_tools))
            .collect();
    }
    if instructions_dir.is_none()
        && sources
            .iter()
            .all(|s| s.dataset.stats.files == 0 && s.dataset.stats.compressed_files_unreadable == 0)
    {
        let places: Vec<String> = if sources.is_empty() {
            read_places_for_message()
        } else {
            sources
                .iter()
                .flat_map(|s| s.places.iter())
                .map(|p| p.display().to_string())
                .collect()
        };
        eprintln!("agentmeter: no session logs found in {}", places.join(", "));
        return ExitCode::FAILURE;
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    if interactive {
        return menu::run(sources, now_ms);
    }
    let period = period_label(options.since, now_ms);

    let since_ms = options.since.map(|s| s.start_ms(now_ms));
    let analyses: Vec<Analysis> = sources
        .iter_mut()
        .map(|s| {
            // The project filter applies before the analysis and before the probe chooses its
            // projects.
            if let Some(name) = &options.project {
                s.dataset.retain_project(name);
                if let Some(t) = s.tools.as_mut() {
                    t.retain_project(name);
                }
            }
            let probe = (options.probe && s.tool == "claude")
                .then(|| run_probe(s, since_ms, io::stderr().is_terminal(), instructions_dir));
            analyse_all(s, options, now_ms, probe, instructions_dir, &period)
        })
        .collect();
    if let Some(name) = &options.project
        && instructions_dir.is_none()
        && sources.iter().all(|s| s.dataset.requests.is_empty())
    {
        eprintln!("agentmeter: no project matches `{name}`");
        return ExitCode::FAILURE;
    }
    if let Some(cli::CaptureAction::Save(name)) = &options.capture {
        return captures::save(&sources, options, name);
    }
    if let Some(Check::DataQuality) = options.check {
        return run_check(&sources, &analyses, options.json);
    }
    let reports: Vec<Report> = sources
        .iter()
        .zip(&analyses)
        .map(|(source, a)| report(source, a, &period, options.project.as_deref()))
        .collect();
    if options.ai_report && !options.json {
        println!("{}", render::ai_report_json(&reports));
    } else if options.json {
        println!("{}", render::json(&reports));
    } else {
        let terminal = std::io::stdout().is_terminal();
        let color = terminal && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        let style = Style::new(color, render::terminal_columns().filter(|_| terminal));
        let blocks: Vec<String> = reports.iter().map(|r| render::text(r, style)).collect();
        print!("{}", blocks.join("\n"));
    }
    ExitCode::SUCCESS
}

/// Every analysis the options ask for, including the ones that read more than the logs.
pub fn analyse_all(
    source: &Source,
    options: &Options,
    now_ms: i64,
    probe: Option<Probe>,
    instructions_dir: Option<&Path>,
    period: &str,
) -> Analysis {
    let since_ms = options.since.map(|s| s.start_ms(now_ms));
    let mut analysis = analyse(source, options, now_ms, probe);
    if options.skills {
        analysis.skills = Some(analyse_skills(source, since_ms, analysis.probe.as_ref()));
    }
    if let Some(dir) = instructions_dir {
        analysis.instructions = Some(scan_instructions(source.tool, dir, analysis.probe.as_ref()));
    }
    if options.ai_report {
        analysis.ai_report = Some(ai_report(source, &analysis, options, period));
    }
    analysis
}

/// What the output prints for one source: everything its analysis holds.
pub fn report<'a>(
    source: &'a Source,
    a: &'a Analysis,
    period: &str,
    project: Option<&'a str>,
) -> Report<'a> {
    Report {
        tool: source.tool,
        label: source.label,
        period: period.to_owned(),
        project,
        places: &source.places,
        quality: &a.quality,
        summary: a.summary.as_ref(),
        sessions: a.sessions.as_deref(),
        projects: a.projects.as_deref(),
        findings: a.findings.as_ref(),
        probe: a.probe.as_ref(),
        instructions: a.instructions.as_ref(),
        tool_calls: a.tool_calls.as_ref(),
        transitions: a.transitions.as_ref(),
        compare: a.compare.as_ref(),
        subagents: a.subagents.as_ref(),
        skills: a.skills.as_ref(),
        compaction_causes: a.compaction_causes.as_ref(),
        before_after: a.before_after.as_ref(),
        retention: a.retention.as_ref(),
        ai_report: a.ai_report.as_ref(),
    }
}

/// The AI report of one source, from the analyses the run made.
fn ai_report(
    source: &Source,
    analysis: &Analysis,
    options: &Options,
    period: &str,
) -> agentmeter_core::ai_report::AiReport {
    let not_condensed: Vec<&'static str> = [
        (options.summary, "summary"),
        (options.sessions, "sessions"),
        (options.projects, "projects"),
        (options.probe, "probe"),
        (options.transitions, "transitions"),
        (options.compare.is_some(), "compare"),
        (options.subagents, "subagents"),
        (options.compaction_causes, "compaction_causes"),
        (options.changed_on.is_some(), "before_after"),
        (options.retention, "retention"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok();
    agentmeter_core::ai_report::build(&agentmeter_core::ai_report::Input {
        tool: source.tool,
        project: options.project.as_deref(),
        period,
        quality: &analysis.quality,
        requests_in_period: analysis.requests_in_period,
        findings: analysis.findings.as_ref(),
        tool_calls: analysis.tool_calls.as_ref(),
        instructions: analysis.instructions.as_ref(),
        skills: analysis.skills.as_ref(),
        not_condensed,
        home: home.as_deref(),
        limit: options
            .ai_report_max
            .unwrap_or(agentmeter_core::ai_report::DEFAULT_LIMIT),
    })
}

/// Why a source cannot say which skills were called.
const CODEX_SKILLS_REASON: &str = "Codex logs its tool calls as scripts without a skill name, so skill use is not \
     measurable; its skill directories are not scanned";

/// Skill calls against the installed skills of one source. Claude only: Codex logs no skill
/// calls, so its rule is not measurable.
fn analyse_skills(
    source: &Source,
    since_ms: Option<i64>,
    probe: Option<&Probe>,
) -> agentmeter_core::skills::SkillAnalysis {
    use agentmeter_core::skills::{SkillScan, analyse};
    if source.tool != "claude" {
        let scan = SkillScan {
            rules_version: "not applicable to Codex",
            ..SkillScan::default()
        };
        return analyse(&scan, None, since_ms, None, Some(CODEX_SKILLS_REASON));
    }
    let scan = agentmeter_claude::skills::scan(
        &agentmeter_claude::config_dirs(),
        &session_dirs(&source.dataset, since_ms),
    );
    analyse(&scan, source.tools.as_ref(), since_ms, probe, None)
}

/// Working directories of the sessions that have requests in the period, once each.
fn session_dirs(dataset: &Dataset, since_ms: Option<i64>) -> Vec<PathBuf> {
    let sessions: std::collections::HashSet<&str> = dataset
        .requests
        .iter()
        .filter(|r| since_ms.is_none_or(|s| r.time_ms >= s))
        .map(|r| r.session.as_str())
        .collect();
    let mut dirs: Vec<PathBuf> = dataset
        .session_starts
        .iter()
        .filter(|s| sessions.contains(s.session.as_str()))
        .filter_map(|s| s.cwd.as_deref().map(PathBuf::from))
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// The instruction files of one agent for `dir`; a probe that ran in `dir` confirms the files
/// Claude Code lists as memory files.
fn scan_instructions(tool: &str, dir: &Path, probe: Option<&Probe>) -> InstructionScan {
    if tool != "claude" {
        return agentmeter_codex::instructions::scan(dir, &agentmeter_codex::config_dirs(), None);
    }
    let mut scan =
        agentmeter_claude::instructions::scan(dir, &agentmeter_claude::config_dirs(), None);
    let run = probe.and_then(|p| p.runs.iter().find(|r| r.place == PROBE_INSTRUCTIONS_PLACE));
    if let Some(run) = run {
        for file in scan
            .files
            .iter_mut()
            .filter(|f| f.state == LoadState::Expected)
        {
            if run.memory_files.iter().any(|m| m.label == file.path) {
                file.state = LoadState::Confirmed;
            }
        }
        scan.notes.push(format!(
            "--probe: Claude Code listed {} memory file(s) in this directory; files it did not \
             list stay 'expected'.",
            run.memory_files.len()
        ));
    }
    scan
}

/// Words for the reported period.
fn period_label(since: Option<Since>, now_ms: i64) -> String {
    match since {
        Some(Since::Days(1)) => "last day".to_owned(),
        Some(Since::Days(n)) => format!("last {n} days"),
        Some(s @ Since::Date(_)) => format!("since {}", format_day(s.start_ms(now_ms))),
        None => "all logs".to_owned(),
    }
}

/// `--check data-quality`: one verdict per source, exit 1 if any source fails.
fn run_check(sources: &[Source], analyses: &[Analysis], json: bool) -> ExitCode {
    let results: Vec<(&Source, Vec<String>)> = sources
        .iter()
        .zip(analyses)
        .map(|(s, a)| (s, check::data_quality(&a.quality, a.requests_in_period)))
        .collect();
    let passed = results.iter().all(|(_, reasons)| reasons.is_empty());
    if json {
        println!("{}", render::check_json(&results));
    } else {
        print!("{}", render::check_text(&results));
    }
    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn read_places_for_message() -> Vec<String> {
    agentmeter_claude::config_dirs()
        .iter()
        .map(|d| d.join("projects"))
        .chain(
            agentmeter_codex::config_dirs()
                .iter()
                .map(|h| h.join("sessions")),
        )
        .map(|p| p.display().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmeter_core::model::ContextProbe;

    #[test]
    fn a_probe_in_the_directory_confirms_only_the_files_it_lists() {
        let dir = std::env::temp_dir().join(format!("agentmeter-main-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "a\n").unwrap();
        std::fs::write(dir.join("CLAUDE.local.md"), "b\n").unwrap();
        let listed = dir.join("CLAUDE.md").display().to_string();
        let run = |place: &str| ContextProbe {
            place: place.to_owned(),
            dir: dir.display().to_string(),
            model: None,
            total: 0,
            sent: Vec::new(),
            deferred: Vec::new(),
            memory_files: vec![agentmeter_core::model::ProbeItem {
                label: listed.clone(),
                tokens: 1,
            }],
            mcp_servers: Vec::new(),
        };
        let state = |scan: &InstructionScan, name: &str| {
            scan.files
                .iter()
                .find(|f| f.path == dir.join(name).display().to_string())
                .unwrap()
                .state
        };
        let probe = Probe {
            runs: vec![run(PROBE_INSTRUCTIONS_PLACE)],
            ..Probe::default()
        };
        let scan = scan_instructions("claude", &dir, Some(&probe));
        assert_eq!(state(&scan, "CLAUDE.md"), LoadState::Confirmed);
        assert_eq!(state(&scan, "CLAUDE.local.md"), LoadState::Expected);
        // A probe of another place confirms nothing.
        let other = Probe {
            runs: vec![run("some project")],
            ..Probe::default()
        };
        let scan = scan_instructions("claude", &dir, Some(&other));
        assert_eq!(state(&scan, "CLAUDE.md"), LoadState::Expected);
        let _ = std::fs::remove_dir_all(dir);
    }
}
