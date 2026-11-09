//! Skill calls from a generated log and the installed skills of a generated configuration
//! (`--skills`).

use agentmeter_claude::read_with_tools;
use agentmeter_claude::skills::scan;
use agentmeter_core::skills::{ListState, analyse};
use std::fs;
use std::path::{Path, PathBuf};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn skill(dir: &Path, header: &str) {
    write(
        &dir.join("SKILL.md"),
        &format!("---\n{header}\n---\n\nSECRET-BODY-TEXT\n"),
    );
}

fn call(minute: u32, id: &str, input: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"2026-10-01T10:{minute:02}:00.000Z","sessionId":"s1","message":{{"id":"m{minute}","model":"claude-x","usage":{{"input_tokens":10,"output_tokens":5}},"content":[{{"type":"tool_use","id":"{id}","name":"Skill","input":{input}}}]}}}}"#
    )
}

/// A configuration with user, plugin and project skills and a log that calls some of them.
fn config(name: &str) -> (PathBuf, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("agentmeter-skills-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let config = root.join("config");
    let project = root.join("proj");
    skill(
        &config.join("skills/alpha"),
        "name: alpha\ndescription: \"abcdefghij\"",
    );
    skill(
        &config.join("skills/beta"),
        "name: beta-renamed\ndescription: >\n  folded\n  text",
    );
    skill(
        &config.join("skills/manual"),
        "name: manual\ndisable-model-invocation: true\ndescription: not listed",
    );
    let plugin = root.join("plugcache");
    skill(&plugin.join("skills/gamma"), "description: from a plugin");
    skill(&root.join("offcache/skills/hidden"), "description: x");
    write(
        &config.join("plugins/installed_plugins.json"),
        &format!(
            r#"{{"version":2,"plugins":{{"pl@mk":[{{"installPath":"{}"}}],"off@mk":[{{"installPath":"{}"}}]}}}}"#,
            plugin.display(),
            root.join("offcache").display()
        ),
    );
    write(
        &config.join("settings.json"),
        r#"{"enabledPlugins":{"pl@mk":true,"off@mk":false}}"#,
    );
    skill(
        &project.join(".claude/skills/delta"),
        "description: project skill",
    );
    let mut lines = Vec::new();
    for (i, input) in [
        r#"{"skill":"alpha","args":"SECRET-ARGS"}"#,
        r#"{"skill":"alpha"}"#,
        r#"{"skill":"alpha"}"#,
        r#"{"skill":"pl:gamma"}"#,
        r#"{"skill":"beta"}"#,
        r#"{"skill":"ghost"}"#,
    ]
    .iter()
    .enumerate()
    {
        lines.push(call(i as u32, &format!("t{i}"), input));
    }
    write(
        &config.join("projects/-Users-me-GitHub-demo/s1.jsonl"),
        &(lines.join("\n") + "\n"),
    );
    (config, project)
}

#[test]
fn the_scan_finds_user_plugin_and_project_skills_with_listing_sizes_and_states() {
    let (config, project) = config("scan");
    let s = scan(std::slice::from_ref(&config), &[project]);
    let entry = |name: &str| {
        s.entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name} not found"))
    };
    // Name plus the length of the description, nothing of the body.
    assert_eq!(entry("alpha").bytes, "alpha".len() as u64 + 10);
    assert_eq!(
        entry("beta-renamed").bytes,
        "beta-renamed".len() as u64 + 11
    );
    assert_eq!(entry("beta-renamed").alias.as_deref(), Some("beta"));
    assert_eq!(entry("manual").state, ListState::UserOnly);
    assert_eq!(entry("manual").bytes, 0);
    assert_eq!(entry("pl:gamma").source, "plugin");
    assert_eq!(entry("delta").source, "project");
    assert!(
        s.entries.iter().all(|e| e.name != "off:hidden"),
        "a plugin the user settings switch off is left out"
    );
    assert!(s.notes.iter().any(|n| n.contains("enabledPlugins")));
    assert!(s.notes.iter().any(|n| n.contains("Not scanned")));
    let _ = fs::remove_dir_all(config.parent().unwrap());
}

#[test]
fn calls_count_by_name_or_directory_and_unknown_names_stay_unscanned() {
    let (config, project) = config("analyse");
    let (_, data) = read_with_tools(std::slice::from_ref(&config), true);
    let s = scan(std::slice::from_ref(&config), &[project]);
    let a = analyse(&s, Some(&data), None, None, None);
    assert_eq!(a.status, "observed");
    assert_eq!(a.calls, 6);
    let calls = |n: &str| a.skills.iter().find(|r| r.name == n).unwrap().calls;
    assert_eq!(calls("alpha"), 3);
    assert_eq!(calls("pl:gamma"), 1);
    assert_eq!(calls("beta-renamed"), 1, "called by its directory name");
    assert_eq!(calls("delta"), 0);
    assert_eq!(a.used, 3);
    assert_eq!(a.unused_listed, 1);
    assert_eq!(a.largest_unused[0].name, "delta");
    assert_eq!(a.called_not_scanned.len(), 1);
    assert_eq!(a.called_not_scanned[0].name, "ghost");
    // A period after every call has no tool calls to judge by.
    let later = analyse(&s, Some(&data), Some(i64::MAX / 2), None, None);
    assert_eq!(later.status, "not_measurable");
    // Neither skill arguments nor skill bodies reach the analysis.
    let json = serde_json::to_string(&a).unwrap();
    assert!(!json.contains("SECRET"));
    let _ = fs::remove_dir_all(config.parent().unwrap());
}

#[test]
fn missing_directories_and_unreadable_files_stay_visible() {
    let root = std::env::temp_dir().join(format!("agentmeter-skills-gaps-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let s = scan(&[root.join("config")], &[root.join("gone")]);
    assert!(s.entries.is_empty());
    assert!(s.notes.iter().any(|n| n.contains("no skills directory")));
    assert!(s.notes.iter().any(|n| n.contains("no plugin list")));
    assert!(s.notes.iter().any(|n| n.contains("no longer exist")));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = root.join("config/skills/locked");
        skill(&dir, "description: x");
        fs::set_permissions(dir.join("SKILL.md"), fs::Permissions::from_mode(0o000)).unwrap();
        let s = scan(&[root.join("config")], &[]);
        let locked = s.entries.iter().find(|e| e.name == "locked").unwrap();
        // A user who can read anyway (root) sees it as listed; otherwise it is unreadable.
        if locked.state == ListState::Unreadable {
            assert!(s.notes.iter().any(|n| n.contains("could not read")));
        }
        let a = analyse(&s, None, None, None, Some("test"));
        assert_eq!(a.installed, 1);
    }
    let _ = fs::remove_dir_all(root);
}
