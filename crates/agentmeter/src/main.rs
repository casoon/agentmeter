//! `agentmeter` — diagnosis for coding-agent usage.
//!
//! Reads the local Claude Code and Codex CLI session logs and reports totals and findings.
//! Read-only.

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
use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// Tool calls; only read with `--tool-calls`.
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
}

pub fn read_sources(tool: Tool) -> Vec<Source> {
    read_sources_with(tool, false)
}

pub fn read_sources_with(tool: Tool, tool_calls: bool) -> Vec<Source> {
    let mut sources = Vec::new();
    if matches!(tool, Tool::All | Tool::Claude) {
        let dirs = agentmeter_claude::config_dirs();
        let places = dirs.iter().map(|d| d.join("projects")).collect();
        let (dataset, tools) = agentmeter_claude::read_with_tools(&dirs, tool_calls);
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
        let (dataset, tools) = agentmeter_codex::read_with_tools(&homes, tool_calls);
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
        compare: options.compare.map(|days| {
            let (earlier, later) = agentmeter_core::compare::windows_before(now_ms, days);
            agentmeter_core::compare::compare(dataset, earlier, later)
        }),
        transitions: options.transitions.then(|| {
            agentmeter_core::transitions::analyse(dataset, source.tools.as_ref(), since_ms)
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

fn run(options: &Options, interactive: bool) -> ExitCode {
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
    let mut sources = read_sources_with(options.tool, options.tool_calls);
    // A file scan needs no logs: the sources are the agents asked for.
    if instructions_dir.is_some() && options.tool == Tool::All {
        sources = read_sources_with(Tool::Claude, options.tool_calls)
            .into_iter()
            .chain(read_sources_with(Tool::Codex, options.tool_calls))
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
            let mut analysis = analyse(s, options, now_ms, probe);
            if let Some(dir) = instructions_dir {
                analysis.instructions =
                    Some(scan_instructions(s.tool, dir, analysis.probe.as_ref()));
            }
            analysis
        })
        .collect();
    if let Some(name) = &options.project
        && instructions_dir.is_none()
        && sources.iter().all(|s| s.dataset.requests.is_empty())
    {
        eprintln!("agentmeter: no project matches `{name}`");
        return ExitCode::FAILURE;
    }
    if let Some(Check::DataQuality) = options.check {
        return run_check(&sources, &analyses, options.json);
    }
    let reports: Vec<Report> = sources
        .iter()
        .zip(&analyses)
        .map(|(source, a)| Report {
            tool: source.tool,
            label: source.label,
            period: period.clone(),
            project: options.project.as_deref(),
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
        })
        .collect();
    if options.json {
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
