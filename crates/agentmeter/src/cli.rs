//! Command line parsing, by hand: the line is a handful of flags, no subcommands.

use agentmeter_core::time::{DAY_MS, parse_date_ms};

/// Where the reported period starts.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Since {
    /// The last N days, counted back from now.
    Days(i64),
    /// 00:00 UTC of a date, in milliseconds since the epoch.
    Date(i64),
}

impl Since {
    pub fn start_ms(self, now_ms: i64) -> i64 {
        match self {
            Since::Days(days) => now_ms - days * DAY_MS,
            Since::Date(ms) => ms,
        }
    }
}

/// Which agent's logs to read.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum Tool {
    /// Every source that has data.
    #[default]
    All,
    Claude,
    Codex,
}

/// What `--check` evaluates. Only conditions named here are checked.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Check {
    /// Unreadable data or no analysable data; never a rule.
    DataQuality,
}

/// Values `--check` accepts, for the error message.
const CHECKS: &str = "data-quality";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub check: Option<Check>,
    pub tool: Tool,
    pub summary: bool,
    pub sessions: bool,
    pub projects: bool,
    pub findings: bool,
    pub since: Option<Since>,
    pub project: Option<String>,
    pub json: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    Run(Options),
    Help,
    Version,
    /// Something the line got wrong; the text says what.
    Error(String),
}

fn parse_since(value: &str) -> Option<Since> {
    if let Some(days) = value.strip_suffix('d') {
        // A day count whose milliseconds do not fit is no period either.
        return days
            .parse::<i64>()
            .ok()
            .filter(|d| *d > 0 && d.checked_mul(DAY_MS).is_some())
            .map(Since::Days);
    }
    parse_date_ms(value).map(Since::Date)
}

pub fn parse<I, S>(args: I) -> Invocation
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let mut options = Options::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Invocation::Help,
            "-V" | "--version" => return Invocation::Version,
            "--summary" => options.summary = true,
            "--sessions" => options.sessions = true,
            "--projects" => options.projects = true,
            "--findings" => options.findings = true,
            "--json" => options.json = true,
            "--since" => match args.next() {
                Some(value) => match parse_since(&value) {
                    Some(since) => options.since = Some(since),
                    None => {
                        return Invocation::Error(format!(
                            "`{value}` is not a period; use 7d or a date like 2026-10-01"
                        ));
                    }
                },
                None => return Invocation::Error("--since needs a value".to_owned()),
            },
            "--check" => match args.next().as_deref() {
                Some("data-quality") => options.check = Some(Check::DataQuality),
                Some(other) => {
                    return Invocation::Error(format!(
                        "`{other}` is not a check; the only check is: {CHECKS}"
                    ));
                }
                None => {
                    return Invocation::Error(format!("--check needs a check: {CHECKS}"));
                }
            },
            "--tool" => match args.next().as_deref() {
                Some("claude") => options.tool = Tool::Claude,
                Some("codex") => options.tool = Tool::Codex,
                Some("all") => options.tool = Tool::All,
                _ => return Invocation::Error("--tool needs claude, codex or all".to_owned()),
            },
            "--project" => match args.next() {
                Some(value) => options.project = Some(value),
                None => return Invocation::Error("--project needs a name".to_owned()),
            },
            _ => return Invocation::Error(format!("unknown option `{arg}`")),
        }
    }
    if !(options.sessions || options.projects || options.findings) {
        options.summary = true;
    }
    Invocation::Run(options)
}

pub fn help(version: &str) -> String {
    format!(
        "agentmeter {version} — diagnosis for coding-agent usage

Usage:
  agentmeter [options]

Without options on a terminal agentmeter opens a menu; anywhere else it prints the summary.

Reads the Claude Code (~/.claude, or CLAUDE_CONFIG_DIR) and Codex CLI (~/.codex, or
CODEX_HOME) session logs and reports totals and findings with next steps.
Read-only, no network. Each source is reported on its own.

Options:
      --summary          Totals, days and compactions (default)
      --sessions         The sessions with the most input
      --projects         Input per project
      --findings         Rules with measured values and tips
      --since <period>   Only requests from the last N days (7d) or from a date (2026-10-01)
      --tool <name>      claude, codex or all (default: every source that has data)
      --project <name>   Only projects whose name contains <name>
      --json             Print JSON instead of text (schema: docs/reference/json.md)
      --check <name>     data-quality: exit 1 if a source has unreadable data or none to
                         analyse, 0 otherwise, 2 for a bad call. Findings never fail it.
  -h, --help             Show this help
  -V, --version          Show the version"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Invocation {
        parse(args.iter().copied())
    }

    #[test]
    fn no_flag_means_summary() {
        let Invocation::Run(options) = run(&[]) else {
            panic!("expected a run");
        };
        assert!(options.summary && !options.findings);
    }

    #[test]
    fn flags_combine_and_take_values() {
        let Invocation::Run(options) =
            run(&["--findings", "--since", "7d", "--project", "x", "--json"])
        else {
            panic!("expected a run");
        };
        assert!(options.findings && !options.summary && options.json);
        assert_eq!(options.since, Some(Since::Days(7)));
        assert_eq!(options.project.as_deref(), Some("x"));
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(matches!(run(&["--since", "soon"]), Invocation::Error(_)));
        assert!(matches!(run(&["--since"]), Invocation::Error(_)));
        assert!(matches!(
            run(&["--since", "99999999999999999d"]),
            Invocation::Error(_)
        ));
        assert!(matches!(run(&["--nope"]), Invocation::Error(_)));
        assert_eq!(run(&["-V"]), Invocation::Version);
    }

    #[test]
    fn check_accepts_only_known_names() {
        let Invocation::Run(options) = run(&["--check", "data-quality"]) else {
            panic!("expected a run");
        };
        assert_eq!(options.check, Some(Check::DataQuality));
        let Invocation::Error(message) = run(&["--check", "findings"]) else {
            panic!("expected an error");
        };
        assert!(message.contains("data-quality"));
        assert!(matches!(run(&["--check"]), Invocation::Error(_)));
    }

    #[test]
    fn dates_are_accepted() {
        assert!(matches!(
            run(&["--since", "2026-10-01"]),
            Invocation::Run(Options {
                since: Some(Since::Date(_)),
                ..
            })
        ));
    }
}
