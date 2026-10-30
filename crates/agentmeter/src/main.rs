//! `agentmeter` — diagnosis for coding-agent usage.
//!
//! Reads the local Claude Code and Codex CLI session logs and reports totals and findings.
//! Read-only.

mod cli;
mod menu;
mod render;

use agentmeter_core::model::Dataset;
use agentmeter_core::report::{ProjectRow, SessionRow, Summary, projects, sessions, summarize};
use agentmeter_core::time::format_day;
use agentmeter_core::{Compaction, DataQuality, Evaluation, Request, check, evaluate};
use cli::{Check, Invocation, Options, Since, Tool};
use render::{Report, Style};
use std::io::{self, IsTerminal};
use std::path::PathBuf;
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
}

pub fn read_sources(tool: Tool) -> Vec<Source> {
    let mut sources = Vec::new();
    if matches!(tool, Tool::All | Tool::Claude) {
        let dirs = agentmeter_claude::config_dirs();
        let places = dirs.iter().map(|d| d.join("projects")).collect();
        let dataset = agentmeter_claude::read(&dirs);
        sources.push(Source {
            tool: "claude",
            label: "Claude Code",
            places,
            dataset,
        });
    }
    if matches!(tool, Tool::All | Tool::Codex) {
        let homes = agentmeter_codex::config_dirs();
        let places = homes.iter().map(|h| h.join("sessions")).collect();
        let dataset = agentmeter_codex::read(&homes);
        sources.push(Source {
            tool: "codex",
            label: "Codex CLI",
            places,
            dataset,
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

pub fn analyse(source: &mut Source, options: &Options, now_ms: i64) -> Analysis {
    if let Some(name) = &options.project {
        source.dataset.retain_project(name);
    }
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
        findings: options.findings.then(|| evaluate(dataset, since_ms)),
    }
}

fn run(options: &Options, interactive: bool) -> ExitCode {
    let mut sources = read_sources(options.tool);
    if sources
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

    let analyses: Vec<Analysis> = sources
        .iter_mut()
        .map(|s| analyse(s, options, now_ms))
        .collect();
    if let Some(name) = &options.project {
        if sources.iter().all(|s| s.dataset.requests.is_empty()) {
            eprintln!("agentmeter: no project matches `{name}`");
            return ExitCode::FAILURE;
        }
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
            quality: &a.quality,
            summary: a.summary.as_ref(),
            sessions: a.sessions.as_deref(),
            projects: a.projects.as_deref(),
            findings: a.findings.as_ref(),
        })
        .collect();
    if options.json {
        println!("{}", render::json(&reports));
    } else {
        let color = std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        let style = Style { color };
        let blocks: Vec<String> = reports.iter().map(|r| render::text(r, style)).collect();
        print!("{}", blocks.join("\n"));
    }
    ExitCode::SUCCESS
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
