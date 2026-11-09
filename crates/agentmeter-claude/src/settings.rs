//! `--compaction-settings`: the auto-compact keys of the Claude Code settings files, as they are
//! today. Read only on request; nothing else in the files is looked at.
//!
//! Files (Claude Code settings docs, as of 2026-11): `settings.json` of each configuration
//! directory (user), and `.claude/settings.json` (project) and `.claude/settings.local.json`
//! (local) in the working directory of each project's latest session, where it still exists.
//! Managed settings, the environment variable and the command-line flag are not read.

use agentmeter_core::compaction::{
    Settings, SettingsFile, SwitchSetting, WindowSetting, window_value,
};
use agentmeter_core::gaps::{Retention, RetentionValue};
use agentmeter_core::model::Dataset;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The working directory of each project's latest session that still is a directory.
fn project_dirs(dataset: &Dataset) -> Vec<(String, PathBuf)> {
    // session -> (project, first request time)
    let mut sessions: HashMap<&str, (&str, i64)> = HashMap::new();
    for r in dataset.requests.iter().filter(|r| !r.sub) {
        let e = sessions
            .entry(r.session.as_str())
            .or_insert((r.project.as_str(), r.time_ms));
        e.1 = e.1.min(r.time_ms);
    }
    let mut latest: HashMap<&str, (i64, PathBuf)> = HashMap::new();
    for s in &dataset.session_starts {
        let (Some(cwd), Some((project, time))) =
            (s.cwd.as_deref(), sessions.get(s.session.as_str()))
        else {
            continue;
        };
        let dir = PathBuf::from(cwd);
        if dir.is_dir() && latest.get(project).is_none_or(|(t, _)| time > t) {
            latest.insert(project, (*time, dir));
        }
    }
    let mut out: Vec<(String, PathBuf)> = latest
        .into_iter()
        .map(|(p, (_, d))| (p.to_owned(), d))
        .collect();
    out.sort();
    out
}

pub fn read(config_dirs: &[PathBuf], dataset: &Dataset) -> Settings {
    let mut settings = Settings::new();
    for dir in config_dirs {
        read_file(&mut settings, &dir.join("settings.json"), "user", None);
    }
    let dirs = project_dirs(dataset);
    let projects: std::collections::BTreeSet<&str> = dataset
        .requests
        .iter()
        .filter(|r| !r.sub)
        .map(|r| r.project.as_str())
        .collect();
    settings.projects_not_read = projects
        .into_iter()
        .filter(|p| !dirs.iter().any(|(d, _)| d == p))
        .map(str::to_owned)
        .collect();
    for (project, dir) in dirs {
        for (name, scope) in [
            ("settings.json", "project"),
            ("settings.local.json", "local"),
        ] {
            read_file(
                &mut settings,
                &dir.join(".claude").join(name),
                scope,
                Some(&project),
            );
        }
    }
    settings
}

/// `--retention`: the `cleanupPeriodDays` key of the same files, compared with the oldest
/// request. Only that key is looked at; managed settings are not read.
pub fn retention(config_dirs: &[PathBuf], dataset: &Dataset, now_ms: i64) -> Retention {
    let mut files = Vec::new();
    let mut values = Vec::new();
    for dir in config_dirs {
        read_retention_file(
            &mut files,
            &mut values,
            &dir.join("settings.json"),
            "user",
            None,
        );
    }
    let dirs = project_dirs(dataset);
    let projects: std::collections::BTreeSet<&str> = dataset
        .requests
        .iter()
        .filter(|r| !r.sub)
        .map(|r| r.project.as_str())
        .collect();
    let projects_not_read = projects
        .into_iter()
        .filter(|p| !dirs.iter().any(|(d, _)| d == p))
        .map(str::to_owned)
        .collect();
    for (project, dir) in dirs {
        for (name, scope) in [
            ("settings.json", "project"),
            ("settings.local.json", "local"),
        ] {
            read_retention_file(
                &mut files,
                &mut values,
                &dir.join(".claude").join(name),
                scope,
                Some(&project),
            );
        }
    }
    let oldest = dataset.requests.iter().map(|r| r.time_ms).min();
    agentmeter_core::gaps::retention(files, values, projects_not_read, oldest, now_ms)
}

fn read_retention_file(
    files: &mut Vec<SettingsFile>,
    values: &mut Vec<RetentionValue>,
    path: &Path,
    scope: &'static str,
    project: Option<&str>,
) {
    let file = path.display().to_string();
    let mut state = |state: &'static str| {
        files.push(SettingsFile {
            path: file.clone(),
            scope,
            project: project.map(str::to_owned),
            state,
        });
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return state("missing"),
        Err(_) => return state("unreadable"),
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return state("invalid_json");
    };
    state("read");
    if let Some(v) = value.get("cleanupPeriodDays") {
        values.push(RetentionValue {
            file,
            scope,
            project: project.map(str::to_owned),
            days: v.as_u64().filter(|d| *d >= 1),
        });
    }
}

fn read_file(settings: &mut Settings, path: &Path, scope: &'static str, project: Option<&str>) {
    let file = path.display().to_string();
    let mut file_state = |state: &'static str| {
        settings.files.push(SettingsFile {
            path: file.clone(),
            scope,
            project: project.map(str::to_owned),
            state,
        });
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return file_state("missing"),
        Err(_) => return file_state("unreadable"),
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return file_state("invalid_json");
    };
    file_state("read");
    let mut add = |model: Option<&str>, v: &serde_json::Value| {
        // An unusable value is ignored by Claude Code as well.
        if let Some((tokens, note)) = window_value(v) {
            settings.windows.push(WindowSetting {
                file: file.clone(),
                scope,
                project: project.map(str::to_owned),
                model: model.map(str::to_owned),
                tokens,
                note,
            });
        }
    };
    if let Some(v) = value.get("autoCompactWindow") {
        add(None, v);
    }
    if let Some(models) = value.get("modelSettings").and_then(|m| m.as_object()) {
        for (model, entry) in models {
            if let Some(v) = entry.get("autoCompactWindow") {
                add(Some(model), v);
            }
        }
    }
    if let Some(enabled) = value.get("autoCompactEnabled").and_then(|v| v.as_bool()) {
        settings.auto_compact_enabled.push(SwitchSetting {
            file,
            scope,
            project: project.map(str::to_owned),
            enabled,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmeter_core::model::{Request, SessionStart};

    #[test]
    fn reads_the_keys_and_keeps_missing_and_broken_files_visible() {
        let root = std::env::temp_dir().join(format!("agentmeter-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cfg = root.join("cfg");
        let proj = root.join("proj");
        fs::create_dir_all(&cfg).unwrap();
        fs::create_dir_all(proj.join(".claude")).unwrap();
        fs::write(
            cfg.join("settings.json"),
            r#"{"autoCompactWindow":300000,"autoCompactEnabled":false,"other":1,
                "modelSettings":{"claude-opus-4-5":{"autoCompactWindow":"auto"},"x":{"effortLevel":"low"}}}"#,
        )
        .unwrap();
        fs::write(proj.join(".claude/settings.local.json"), "{ not json").unwrap();
        let mut dataset = Dataset::default();
        dataset.requests.push(Request {
            session: "s".into(),
            project: "p".into(),
            source: 0,
            message_id: String::new(),
            time_ms: 1,
            model: "m".into(),
            sub: false,
            fresh: 1,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            output: 1,
            window: None,
        });
        dataset.session_starts.push(SessionStart {
            session: "s".into(),
            source: 0,
            clean: true,
            reason: None,
            entrypoint: None,
            cwd: Some(proj.display().to_string()),
            parent: None,
        });
        let s = read(std::slice::from_ref(&cfg), &dataset);
        let state = |suffix: &str| {
            s.files
                .iter()
                .find(|f| f.path.ends_with(suffix))
                .unwrap()
                .state
        };
        assert_eq!(state("cfg/settings.json"), "read");
        assert_eq!(state(".claude/settings.json"), "missing");
        assert_eq!(state("settings.local.json"), "invalid_json");
        assert_eq!(s.windows.len(), 2);
        assert_eq!(s.windows[0].tokens, Some(300_000));
        assert_eq!(s.windows[1].model.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(s.windows[1].tokens, None);
        assert!(!s.auto_compact_enabled[0].enabled);
        assert!(s.projects_not_read.is_empty());

        // Retention: no key gives the default; an unusable key gives none; a key is used.
        let r = retention(std::slice::from_ref(&cfg), &dataset, 40 * 86_400_000);
        assert_eq!(r.effective_days, Some(30));
        fs::write(cfg.join("settings.json"), r#"{"cleanupPeriodDays":0}"#).unwrap();
        let r = retention(std::slice::from_ref(&cfg), &dataset, 40 * 86_400_000);
        assert_eq!((r.effective_days, r.values.len()), (None, 1));
        fs::write(cfg.join("settings.json"), r#"{"cleanupPeriodDays":45}"#).unwrap();
        let r = retention(&[cfg], &dataset, 40 * 86_400_000);
        assert_eq!((r.effective_days, r.basis), (Some(45), Some("settings")));
        let _ = fs::remove_dir_all(root);
    }
}
