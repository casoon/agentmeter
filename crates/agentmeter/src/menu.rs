//! The interactive menu: a terminal front end over the same analysis the text output uses.
//!
//! The menu is a model first ([`model`]: sections and rows as plain data, testable without a
//! terminal) and a thin layer that hands the rows to `runemark`. The rows are the reports of
//! `--report`: a question each, answered by several analyses at once. Choosing one prints what
//! the command line would print and returns to the menu.

use crate::cli::{Bundle, Options, Since, Tool};
use crate::render::{self, Style};
use crate::{Analysis, Source, analyse, analyse_all, period_label, read_sources_with, run_probe};
use agentmeter_core::format::{count, tokens};
use agentmeter_core::model::Probe;
use runemark::{ColorMode, Console, ErrorBlock, Group, Hint, Item, Menu, Outcome, SelectMode};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

/// The periods the menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Days7,
    Days30,
    All,
}

impl Period {
    pub fn next(self) -> Period {
        match self {
            Period::Days7 => Period::Days30,
            Period::Days30 => Period::All,
            Period::All => Period::Days7,
        }
    }

    /// The days "What changed?" puts next to the days before; all logs have no such length.
    pub fn compare_days(self) -> i64 {
        match self {
            Period::Days30 => 30,
            Period::Days7 | Period::All => 7,
        }
    }

    pub fn since(self) -> Option<Since> {
        match self {
            Period::Days7 => Some(Since::Days(7)),
            Period::Days30 => Some(Since::Days(30)),
            Period::All => None,
        }
    }
}

/// One selectable line.
#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub label: String,
    pub description: String,
}

/// A titled group of rows.
#[derive(Debug, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

/// A source with its analysis, as the model sees it.
pub struct View<'a> {
    pub source: &'a Source,
    pub analysis: &'a Analysis,
}

/// `claude: …; codex: …` when there is more than one source, the bare text otherwise.
fn per_source(views: &[View], text: impl Fn(&View) -> String) -> String {
    if views.len() == 1 {
        return text(&views[0]);
    }
    views
        .iter()
        .map(|v| format!("{} {}", v.source.tool, text(v)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// How many kinds of notes the data quality of a source has to show.
fn data_notes(a: &Analysis) -> usize {
    let r = &a.quality.read;
    [
        r.files_without_usage > 0,
        r.skipped_lines > 0,
        !r.unknown_types.is_empty(),
        !r.unknown_usage_fields.is_empty(),
        r.compressed_files_unreadable > 0,
        r.usage_delta_mismatches > 0,
        r.requests_without_model > 0,
        r.compactions_without_post_tokens > 0,
        !r.cache_write_measured && a.quality.requests > 0,
    ]
    .iter()
    .filter(|note| **note)
    .count()
}

/// The menu as data: REPORTS (one row per question) and DATA (one row per source).
pub fn model(views: &[View], period: Period) -> Vec<Section> {
    let row = |id: String, label: String, description: String| Row {
        id,
        label,
        description,
    };
    let reports = Bundle::ALL
        .into_iter()
        .map(|bundle| {
            let description = match bundle {
                Bundle::Overview => overview_words(views),
                Bundle::Changes => {
                    let days = period.compare_days();
                    format!("last {days} days next to the {days} before")
                }
                // Asking Claude Code starts a program; the row says so before it is chosen.
                Bundle::Start if views.iter().any(|v| v.source.tool == "claude") => {
                    format!("{}, runs claude for /context", bundle.about())
                }
                _ => bundle.about().to_owned(),
            };
            row(
                format!("report:{}", bundle.name()),
                bundle.title().to_owned(),
                description,
            )
        })
        .collect();
    let data = views
        .iter()
        .map(|v| {
            let description = if v.analysis.quality.analysable {
                format!(
                    "{} requests · {} notes",
                    count(v.analysis.quality.requests),
                    data_notes(v.analysis)
                )
            } else {
                "no analysable data".to_owned()
            };
            row(
                format!("data:{}", v.source.tool),
                format!("Data · {}", v.source.label),
                description,
            )
        })
        .collect();
    vec![
        Section {
            title: "REPORTS",
            rows: reports,
        },
        Section {
            title: "DATA",
            rows: data,
        },
    ]
}

/// The overview row: what the period holds, before anything is chosen.
fn overview_words(views: &[View]) -> String {
    let observed = |v: &View| {
        v.analysis.findings.as_ref().map_or(0, |e| {
            e.rules.iter().filter(|r| r.status == "observed").count()
        })
    };
    if views.len() > 1 {
        let counts = per_source(views, |v| {
            count(v.analysis.summary.as_ref().map_or(0, |s| s.requests))
        });
        return format!("{counts} requests");
    }
    match &views[0].analysis.summary {
        Some(s) if s.requests > 0 => format!(
            "{} requests · {} input · {} findings",
            count(s.requests),
            tokens(s.input),
            observed(&views[0])
        ),
        _ => "no requests in the period".to_owned(),
    }
}

/// The options a row stands for: a report with the menu's source and period, or the data
/// quality alone for a `data:` row.
fn row_options(id: &str, tool: Tool, period: Period) -> Options {
    let mut options = Options {
        since: period.since(),
        tool,
        ..Options::default()
    };
    let bundle = id
        .strip_prefix("report:")
        .and_then(|name| Bundle::ALL.into_iter().find(|b| b.name() == name));
    if let Some(bundle) = bundle {
        if bundle == Bundle::Changes {
            // The comparison sets both periods itself.
            options.since = None;
            options.compare = Some(period.compare_days());
        }
        options.report = Some(bundle);
        bundle.apply(&mut options);
    }
    options
}

/// The output for a row: what the command line prints for the same options.
fn detail(
    id: &str,
    sources: &[Source],
    options: &Options,
    now_ms: i64,
    probe: Option<&Probe>,
    style: Style,
) -> String {
    let period = period_label(options.since, now_ms);
    let dir = options.instructions.as_deref().map(Path::new);
    let chosen: Vec<&Source> = sources
        .iter()
        .filter(|s| options.tool == Tool::All || s.tool == tool_name(options.tool))
        .filter(|s| id.strip_prefix("data:").is_none_or(|tool| tool == s.tool))
        .collect();
    let analyses: Vec<Analysis> = chosen
        .iter()
        .map(|s| {
            let probe = probe.cloned().filter(|_| s.tool == "claude");
            analyse_all(s, options, now_ms, probe, dir, &period)
        })
        .collect();
    let reports: Vec<_> = chosen
        .iter()
        .zip(&analyses)
        .map(|(source, a)| crate::report(source, a, &period, None))
        .collect();
    if options.ai_report {
        return format!("{}\n", render::ai_report_json(&reports));
    }
    let blocks: Vec<String> = reports.iter().map(|r| render::text(r, style)).collect();
    blocks.join("\n")
}

/// The choices for the source: all (if more than one) and each source on its own.
fn source_choices(sources: &[Source]) -> Vec<Tool> {
    let mut choices = Vec::new();
    if sources.len() > 1 {
        choices.push(Tool::All);
    }
    for source in sources {
        choices.push(if source.tool == "claude" {
            Tool::Claude
        } else {
            Tool::Codex
        });
    }
    choices
}

fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::All => "claude",
        Tool::Claude => "claude",
        Tool::Codex => "codex",
    }
}

fn to_menu(sections: &[Section], note: String) -> Menu {
    let mut menu = Menu::new().with_heading("agentmeter").with_note(note);
    for section in sections {
        let mut group = Group::new(section.title);
        for row in &section.rows {
            group =
                group.add_item(Item::new(&row.id, &row.label).with_description(&row.description));
        }
        menu = menu.add_group(group);
    }
    menu.add_hint(Hint::new('S', "Source"))
        .add_hint(Hint::new('P', "Period"))
}

/// Runs the menu until the user leaves it.
pub fn run(mut sources: Vec<Source>, now_ms: i64) -> ExitCode {
    let console = Console::stderr(ColorMode::Auto);
    let choices = source_choices(&sources);
    let (mut choice, mut period) = (0usize, Period::Days7);
    // Tool calls are read the first time a report asks for them, the probe runs once; both
    // then serve every later report.
    let mut with_tools = false;
    let mut probe: Option<Probe> = None;
    let style = Style::new(
        std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
        render::terminal_columns(),
    );

    loop {
        let wanted = choices[choice];
        let chosen = |s: &Source| wanted == Tool::All || s.tool == tool_name(wanted);
        let outcome = {
            // What the rows say about the period: totals and findings, no message content.
            let options = Options {
                summary: true,
                findings: true,
                since: period.since(),
                tool: wanted,
                ..Options::default()
            };
            let analyses: Vec<Analysis> = sources
                .iter()
                .filter(|s| chosen(s))
                .map(|s| analyse(s, &options, now_ms, None))
                .collect();
            let views: Vec<View> = sources
                .iter()
                .filter(|s| chosen(s))
                .zip(&analyses)
                .map(|(source, analysis)| View { source, analysis })
                .collect();
            let labels: Vec<&str> = views.iter().map(|v| v.source.label).collect();
            let note = format!(
                "{} · {}",
                labels.join(" · "),
                period_label(period.since(), now_ms)
            );
            to_menu(&model(&views, period), note).run(console, SelectMode::Always, true)
        };
        match outcome {
            Ok(Outcome::Selected(id)) => {
                let options = row_options(&id, wanted, period);
                if (options.tool_calls || options.skills) && !with_tools {
                    eprintln!("Reading tool calls …");
                    sources = read_sources_with(Tool::All, true);
                    with_tools = true;
                }
                if options.report == Some(Bundle::Start)
                    && probe.is_none()
                    && let Some(claude) = sources.iter().find(|s| chosen(s) && s.tool == "claude")
                {
                    let since_ms = options.since.map(|s| s.start_ms(now_ms));
                    let dir = options.instructions.as_deref().map(Path::new);
                    probe = Some(run_probe(claude, since_ms, true, dir));
                }
                let probe = probe
                    .as_ref()
                    .filter(|_| options.report == Some(Bundle::Start));
                print!("{}", detail(&id, &sources, &options, now_ms, probe, style));
                eprint!("\nPress Enter to return to the menu. ");
                let _ = io::stderr().flush();
                let mut line = String::new();
                let _ = io::stdin().lock().read_line(&mut line);
            }
            Ok(Outcome::Hotkey('S')) => choice = (choice + 1) % choices.len(),
            Ok(Outcome::Hotkey('P')) => period = period.next(),
            Ok(Outcome::Hotkey(_)) => {}
            Ok(Outcome::Cancelled) | Ok(Outcome::Unavailable) => return ExitCode::SUCCESS,
            Err(error) => {
                let block = ErrorBlock::new("Cannot open the terminal")
                    .with_explanation(format!("{error}"))
                    .with_remedy("Run agentmeter --summary for the text output.");
                eprint!("{}", block.render(console));
                return ExitCode::FAILURE;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    fn source(tool: &'static str, label: &'static str, dir: &str) -> Source {
        let dataset = if tool == "claude" {
            agentmeter_claude::read(&[fixture(dir)])
        } else {
            agentmeter_codex::read(&[fixture(dir)])
        };
        Source {
            tool,
            label,
            places: Vec::new(),
            dataset,
            tools: None,
        }
    }

    fn analysed(mut sources: Vec<Source>) -> (Vec<Source>, Vec<Analysis>) {
        let options = Options {
            summary: true,
            findings: true,
            ..Options::default()
        };
        let analyses = sources
            .iter_mut()
            .map(|s| analyse(s, &options, 0, None))
            .collect();
        (sources, analyses)
    }

    fn views<'a>(sources: &'a [Source], analyses: &'a [Analysis]) -> Vec<View<'a>> {
        sources
            .iter()
            .zip(analyses)
            .map(|(source, analysis)| View { source, analysis })
            .collect()
    }

    #[test]
    fn model_has_one_row_per_report_and_a_data_row_per_source() {
        let (sources, analyses) = analysed(vec![source("claude", "Claude Code", "epoch-current")]);
        let model = model(&views(&sources, &analyses), Period::Days30);
        let titles: Vec<_> = model.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["REPORTS", "DATA"]);
        let ids: Vec<_> = model[0].rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "report:overview",
                "report:changes",
                "report:waste",
                "report:start",
                "report:ai"
            ]
        );
        assert!(model[0].rows[0].description.starts_with("4 requests"));
        assert!(model[0].rows[0].description.ends_with("findings"));
        assert_eq!(
            model[0].rows[1].description,
            "last 30 days next to the 30 before"
        );
        // A row that opens message content or starts claude says so before it is chosen.
        for row in &model[0].rows[2..] {
            assert!(row.description.contains("reads tool calls"), "{}", row.id);
        }
        assert!(model[0].rows[3].description.contains("runs claude"));
        assert_eq!(model[1].rows.len(), 1);
        assert_eq!(model[1].rows[0].id, "data:claude");
    }

    #[test]
    fn two_sources_share_a_row_and_name_the_tool() {
        let (sources, analyses) = analysed(vec![
            source("claude", "Claude Code", "epoch-current"),
            source("codex", "Codex CLI", "codex-basic"),
        ]);
        let model = model(&views(&sources, &analyses), Period::Days7);
        assert_eq!(model[0].rows[0].description, "claude 4 · codex 2 requests");
        assert_eq!(model[1].rows.len(), 2);
    }

    #[test]
    fn codex_alone_is_not_told_that_claude_runs() {
        let (sources, analyses) = analysed(vec![source("codex", "Codex CLI", "codex-basic")]);
        let model = model(&views(&sources, &analyses), Period::Days7);
        assert!(!model[0].rows[3].description.contains("runs claude"));
    }

    #[test]
    fn source_without_usage_says_so_instead_of_looking_healthy() {
        let (sources, analyses) =
            analysed(vec![source("codex", "Codex CLI", "codex-no-token-count")]);
        let model = model(&views(&sources, &analyses), Period::Days7);
        assert_eq!(model[1].rows[0].description, "no analysable data");
        assert_eq!(model[0].rows[0].description, "no requests in the period");
    }

    #[test]
    fn a_row_stands_for_the_options_of_its_report() {
        let waste = row_options("report:waste", Tool::Claude, Period::Days30);
        assert!(waste.tool_calls && waste.compaction_causes && waste.subagents);
        assert_eq!(waste.since, Some(Since::Days(30)));
        assert_eq!(waste.tool, Tool::Claude);
        // The comparison sets both periods itself, from the menu's period.
        let changes = row_options("report:changes", Tool::All, Period::Days30);
        assert_eq!((changes.since, changes.compare), (None, Some(30)));
        let all = row_options("report:changes", Tool::All, Period::All);
        assert_eq!(all.compare, Some(7));
        // The overview and a data row never open message content.
        for id in ["report:overview", "report:changes", "data:claude"] {
            let options = row_options(id, Tool::All, Period::Days7);
            assert!(!options.tool_calls && !options.skills, "{id}");
        }
        let data = row_options("data:claude", Tool::All, Period::Days7);
        assert!(!data.summary && !data.findings);
    }

    #[test]
    fn detail_prints_what_the_report_holds() {
        let sources = vec![
            source("claude", "Claude Code", "epoch-current"),
            source("codex", "Codex CLI", "codex-basic"),
        ];
        let style = Style::new(false, None);
        let show = |id: &str| {
            let options = row_options(id, Tool::All, Period::All);
            detail(id, &sources, &options, 0, None, style)
        };
        let overview = show("report:overview");
        assert!(overview.contains("SUMMARY") && overview.contains("FINDINGS"));
        assert!(overview.contains("Claude Code") && overview.contains("Codex CLI"));
        let data = show("data:codex");
        assert!(data.contains("Codex CLI") && !data.contains("Claude Code"));
        assert!(!data.contains("SUMMARY"));
        // The AI report is one line of JSON, as on the command line.
        let ai = show("report:ai");
        assert!(ai.starts_with('{') && ai.ends_with("}\n"));
        // The source the menu is set to limits every report.
        let options = row_options("report:overview", Tool::Codex, Period::All);
        let codex = detail("report:overview", &sources, &options, 0, None, style);
        assert!(!codex.contains("Claude Code"));
    }

    #[test]
    fn period_and_source_cycle() {
        assert_eq!(Period::Days7.next().next().next(), Period::Days7);
        assert_eq!(Period::All.since(), None);
        let two = vec![
            source("claude", "Claude Code", "epoch-current"),
            source("codex", "Codex CLI", "codex-basic"),
        ];
        assert_eq!(source_choices(&two), [Tool::All, Tool::Claude, Tool::Codex]);
        assert_eq!(source_choices(&two[..1]), [Tool::Claude]);
    }
}
