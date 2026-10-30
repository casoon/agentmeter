//! The interactive menu: a terminal front end over the same analysis the text output uses.
//!
//! The menu is a model first ([`model`]: sections and rows as plain data, testable without a
//! terminal) and a thin layer that hands the rows to `runemark`. Choosing a row prints the
//! existing detail output and returns to the menu.

use crate::cli::{Options, Since, Tool};
use crate::render::{self, Report, Style};
use crate::{Analysis, Source, analyse, period_label};
use agentmeter_core::format::{count, tokens};
use runemark::{ColorMode, Console, ErrorBlock, Group, Hint, Item, Menu, Outcome, SelectMode};
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

/// The rules, in the order the findings are listed.
const RULES: &[&str] = &[
    "baseline-load",
    "cache-rebuild",
    "compact-thrash",
    "big-context",
    "long-session",
];

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

fn status_words(status: &str) -> &'static str {
    match status {
        "observed" => "observed",
        "checked_none" => "none",
        _ => "n/a",
    }
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

/// The menu as data: OVERVIEW, FINDINGS (one row per rule) and DATA (one row per source).
pub fn model(views: &[View]) -> Vec<Section> {
    let row = |id: &str, label: &str, description: String| Row {
        id: id.to_owned(),
        label: label.to_owned(),
        description,
    };
    let overview = vec![
        row(
            "summary",
            "Summary",
            if views.len() == 1 {
                match &views[0].analysis.summary {
                    Some(s) if s.requests > 0 => format!(
                        "{} requests · {} input · cache {:.0} %",
                        count(s.requests),
                        tokens(s.input),
                        s.cache_read_pct
                    ),
                    _ => "no requests in the period".to_owned(),
                }
            } else {
                let counts = per_source(views, |v| {
                    count(v.analysis.summary.as_ref().map_or(0, |s| s.requests))
                });
                format!("{counts} requests")
            },
        ),
        row(
            "sessions",
            "Sessions",
            per_source(views, |v| {
                format!("{}", v.analysis.sessions.as_ref().map_or(0, Vec::len))
            }),
        ),
        row(
            "projects",
            "Projects",
            per_source(views, |v| {
                format!("{}", v.analysis.projects.as_ref().map_or(0, Vec::len))
            }),
        ),
    ];
    let findings = RULES
        .iter()
        .map(|rule| {
            row(
                &format!("rule:{rule}"),
                rule,
                per_source(views, |v| {
                    v.analysis
                        .findings
                        .as_ref()
                        .and_then(|e| e.rules.iter().find(|r| r.rule == *rule))
                        .map_or("n/a", |r| status_words(r.status))
                        .to_owned()
                }),
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
                &format!("data:{}", v.source.tool),
                &format!("Data · {}", v.source.label),
                description,
            )
        })
        .collect();
    vec![
        Section {
            title: "OVERVIEW",
            rows: overview,
        },
        Section {
            title: "FINDINGS",
            rows: findings,
        },
        Section {
            title: "DATA",
            rows: data,
        },
    ]
}

/// The detail output for a row: the text the flags would print.
fn detail(id: &str, views: &[View], period: &str, style: Style) -> String {
    let blocks: Vec<String> = views
        .iter()
        .filter(|v| {
            id.strip_prefix("data:")
                .is_none_or(|tool| tool == v.source.tool)
        })
        .map(|v| {
            let a = v.analysis;
            let report = Report {
                tool: v.source.tool,
                label: v.source.label,
                period: period.to_owned(),
                project: None,
                quality: &a.quality,
                summary: a.summary.as_ref().filter(|_| id == "summary"),
                sessions: a.sessions.as_deref().filter(|_| id == "sessions"),
                projects: a.projects.as_deref().filter(|_| id == "projects"),
                findings: a.findings.as_ref().filter(|_| id.starts_with("rule:")),
            };
            render::text(&report, style)
        })
        .collect();
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
    let style = Style {
        color: std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
    };

    loop {
        let wanted = choices[choice];
        let options = Options {
            summary: true,
            sessions: true,
            projects: true,
            findings: true,
            since: period.since(),
            tool: wanted,
            ..Options::default()
        };
        let chosen = |s: &Source| wanted == Tool::All || s.tool == tool_name(wanted);
        let analyses: Vec<Analysis> = sources
            .iter_mut()
            .filter(|s| chosen(s))
            .map(|s| analyse(s, &options, now_ms))
            .collect();
        let views: Vec<View> = sources
            .iter()
            .filter(|s| chosen(s))
            .zip(&analyses)
            .map(|(source, analysis)| View { source, analysis })
            .collect();
        let period_words = period_label(period.since(), now_ms);
        let labels: Vec<&str> = views.iter().map(|v| v.source.label).collect();
        let menu = to_menu(
            &model(&views),
            format!("{} · {period_words}", labels.join(" · ")),
        );

        match menu.run(console, SelectMode::Always, true) {
            Ok(Outcome::Selected(id)) => {
                print!("{}", detail(&id, &views, &period_words, style));
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
        }
    }

    fn analysed(mut sources: Vec<Source>) -> (Vec<Source>, Vec<Analysis>) {
        let options = Options {
            summary: true,
            sessions: true,
            projects: true,
            findings: true,
            ..Options::default()
        };
        let analyses = sources
            .iter_mut()
            .map(|s| analyse(s, &options, 0))
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
    fn model_has_overview_one_row_per_rule_and_a_data_row_per_source() {
        let (sources, analyses) = analysed(vec![source("claude", "Claude Code", "epoch-current")]);
        let model = model(&views(&sources, &analyses));
        let titles: Vec<_> = model.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["OVERVIEW", "FINDINGS", "DATA"]);
        assert_eq!(model[0].rows[0].id, "summary");
        assert!(model[0].rows[0].description.starts_with("4 requests"));
        let ids: Vec<_> = model[1].rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), 5);
        assert!(ids.contains(&"rule:cache-rebuild"));
        let thrash = model[1]
            .rows
            .iter()
            .find(|r| r.label == "compact-thrash")
            .unwrap();
        assert!(thrash.description == "none" || thrash.description == "n/a");
        assert_eq!(model[2].rows.len(), 1);
        assert_eq!(model[2].rows[0].id, "data:claude");
    }

    #[test]
    fn two_sources_share_a_row_and_name_the_tool() {
        let (sources, analyses) = analysed(vec![
            source("claude", "Claude Code", "epoch-current"),
            source("codex", "Codex CLI", "codex-basic"),
        ]);
        let model = model(&views(&sources, &analyses));
        assert_eq!(model[0].rows[0].description, "claude 4 · codex 2 requests");
        let rebuild = model[1]
            .rows
            .iter()
            .find(|r| r.label == "cache-rebuild")
            .unwrap();
        assert!(rebuild.description.contains("codex n/a"));
        assert_eq!(model[2].rows.len(), 2);
    }

    #[test]
    fn source_without_usage_says_so_instead_of_looking_healthy() {
        let (sources, analyses) =
            analysed(vec![source("codex", "Codex CLI", "codex-no-token-count")]);
        let model = model(&views(&sources, &analyses));
        assert_eq!(model[2].rows[0].description, "no analysable data");
        assert_eq!(model[0].rows[0].description, "no requests in the period");
    }

    #[test]
    fn detail_prints_only_the_chosen_part() {
        let (sources, analyses) = analysed(vec![
            source("claude", "Claude Code", "epoch-current"),
            source("codex", "Codex CLI", "codex-basic"),
        ]);
        let v = views(&sources, &analyses);
        let style = Style { color: false };
        let summary = detail("summary", &v, "all logs", style);
        assert!(summary.contains("SUMMARY") && !summary.contains("FINDINGS"));
        let data = detail("data:codex", &v, "all logs", style);
        assert!(data.contains("Codex CLI") && !data.contains("Claude Code"));
        assert!(!data.contains("SUMMARY"));
        assert!(detail("rule:big-context", &v, "all logs", style).contains("FINDINGS"));
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
