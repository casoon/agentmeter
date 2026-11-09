//! Local captures: `--capture`, `--captures`, `--compare-captures`, `--delete-capture`.
//!
//! The only place that touches files for them. A capture is written only when asked for, one JSON
//! file per name, and read or deleted only by name. Nothing is tracked automatically.

use crate::cli::{CaptureAction, Options};
use crate::render::{self, Style};
use crate::{Source, now_ms};
use agentmeter_core::capture::{self, Capture};
use agentmeter_core::compare::Window;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `$AGENTMETER_DATA_DIR/captures`, else `$XDG_DATA_HOME/agentmeter/captures`, else
/// `~/.local/share/agentmeter/captures`.
pub fn dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(d) = var("AGENTMETER_DATA_DIR") {
        return Some(PathBuf::from(d).join("captures"));
    }
    if let Some(d) = var("XDG_DATA_HOME") {
        return Some(PathBuf::from(d).join("agentmeter/captures"));
    }
    var("HOME").map(|h| PathBuf::from(h).join(".local/share/agentmeter/captures"))
}

fn style() -> Style {
    let terminal = std::io::stdout().is_terminal();
    let color = terminal && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    Style::new(color, render::terminal_columns().filter(|_| terminal))
}

fn fail(message: String) -> ExitCode {
    eprintln!("agentmeter: {message}");
    ExitCode::FAILURE
}

fn store() -> Result<PathBuf, String> {
    dir().ok_or_else(|| "no place for captures: set AGENTMETER_DATA_DIR".to_owned())
}

fn load(dir: &Path, name: &str) -> Result<Capture, String> {
    let text = std::fs::read_to_string(dir.join(format!("{name}.json")))
        .map_err(|_| format!("no capture `{name}` in {}", dir.display()))?;
    capture::parse(&text).map_err(|e| format!("capture `{name}`: {e}"))
}

/// The actions that need no logs. `None` when `options` asks for none of them.
pub fn store_action(options: &Options) -> Option<ExitCode> {
    match options.capture.as_ref()? {
        CaptureAction::List => Some(list(options.json)),
        CaptureAction::Compare(a, b) => Some(compare(a, b, options.json)),
        CaptureAction::Delete(name) => Some(delete(name, options.json)),
        CaptureAction::Save(_) => None,
    }
}

fn list(json: bool) -> ExitCode {
    let dir = match store() {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let mut captures = Vec::new();
    let mut unreadable = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let name = path
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned());
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| capture::parse(&t))
        {
            Ok(c) => captures.push(c),
            Err(reason) => unreadable.push((name, reason)),
        }
    }
    if json {
        let rows: Vec<_> = captures
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name, "label": c.label, "created": c.created, "tool": c.tool,
                    "project": c.project, "from": c.facts.from, "to": c.facts.to,
                    "requests": c.facts.requests,
                })
            })
            .collect();
        let bad: Vec<_> = unreadable
            .iter()
            .map(|(n, r)| serde_json::json!({"name": n, "reason": r}))
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 1, "directory": dir.display().to_string(),
                "captures": rows, "unreadable": bad,
            })
        );
    } else {
        print!(
            "{}",
            render::captures_text(&dir.display().to_string(), &captures, &unreadable, style())
        );
    }
    ExitCode::SUCCESS
}

fn compare(earlier: &str, later: &str, json: bool) -> ExitCode {
    let result = store().and_then(|dir| Ok((load(&dir, earlier)?, load(&dir, later)?)));
    let (a, b) = match result {
        Ok(pair) => pair,
        Err(e) => return fail(e),
    };
    let c = capture::compare_captures(&a, &b);
    if json {
        println!(
            "{}",
            serde_json::json!({"schema_version": 1, "capture_comparison": c})
        );
    } else {
        print!("{}", render::capture_comparison_text(&c, style()));
    }
    ExitCode::SUCCESS
}

fn delete(name: &str, json: bool) -> ExitCode {
    let path = match store() {
        Ok(d) => d.join(format!("{name}.json")),
        Err(e) => return fail(e),
    };
    if std::fs::remove_file(&path).is_err() {
        return fail(format!("no capture `{name}` at {}", path.display()));
    }
    if json {
        println!(
            "{}",
            serde_json::json!({"schema_version": 1, "deleted": name})
        );
    } else {
        println!("deleted capture `{name}`");
    }
    ExitCode::SUCCESS
}

/// `--capture <name>`: the period since `--since` up to now, of the one source, written to a new
/// file. An existing capture is never overwritten.
pub fn save(sources: &[Source], options: &Options, name: &str) -> ExitCode {
    let [source] = sources else {
        return fail("--capture records one source; choose it with --tool claude or codex".into());
    };
    let Some(since) = options.since else {
        return fail("--capture needs --since".into());
    };
    let dir = match store() {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let path = dir.join(format!("{name}.json"));
    if path.exists() {
        return fail(format!(
            "capture `{name}` exists; delete it first with --delete-capture {name}"
        ));
    }
    let now = now_ms();
    let window = Window {
        from_ms: since.start_ms(now),
        to_ms: now,
    };
    let c = capture::capture(
        &source.dataset,
        source.tool,
        options.project.as_deref(),
        name,
        options.change_label.as_deref(),
        window,
        now,
    );
    let text = serde_json::to_string_pretty(&c).unwrap_or_default() + "\n";
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)) {
        return fail(format!("could not write {}: {e}", path.display()));
    }
    if options.json {
        println!("{}", serde_json::json!(c));
    } else {
        println!(
            "captured `{name}`: {} requests, {} to {}, in {}",
            c.facts.requests,
            &c.facts.from[..10],
            &c.facts.to[..10],
            path.display()
        );
    }
    ExitCode::SUCCESS
}
