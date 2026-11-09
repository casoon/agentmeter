//! The instruction scan against synthetic trees (tests/fixtures/instr and temporary ones).

use agentmeter_claude::instructions::scan;
use agentmeter_core::instructions::{InstructionScan, LoadState};
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/instr")
}

fn state(scan: &InstructionScan, suffix: &str) -> LoadState {
    scan.files
        .iter()
        .find(|f| f.path.ends_with(suffix))
        .unwrap_or_else(|| {
            panic!(
                "{suffix} not listed: {:?}",
                scan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
            )
        })
        .state
}

fn tree(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentmeter-instr-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn user_ancestor_project_nested_rules_and_imports_are_told_apart() {
    let root = fs::canonicalize(fixtures().join("claude")).unwrap();
    let home = fixtures().join("home/.claude");
    let s = scan(&root.join("repo/pkg"), &[home], Some(&root));
    assert_eq!(state(&s, "home/.claude/CLAUDE.md"), LoadState::Expected);
    assert_eq!(state(&s, "pkg/CLAUDE.md"), LoadState::Expected);
    assert_eq!(state(&s, "repo/CLAUDE.md"), LoadState::Expected);
    // Imported by the repo file, resolved relative to it.
    let guide = s
        .files
        .iter()
        .find(|f| f.path.ends_with("docs/guide.md"))
        .unwrap();
    assert_eq!((guide.scope, guide.state), ("import", LoadState::Expected));
    // Below the directory: on demand, not at start.
    assert_eq!(state(&s, "app/CLAUDE.md"), LoadState::OnDemand);
    // A path-scoped user rule is not start context.
    assert_eq!(state(&s, "rules/ts.md"), LoadState::OnDemand);
    // AGENTS.md is shadowed by the CLAUDE.md files.
    assert_eq!(state(&s, "pkg/AGENTS.md"), LoadState::NotLoaded);
}

#[test]
fn files_that_are_not_loaded_do_not_count_as_context() {
    let root = fs::canonicalize(fixtures().join("claude")).unwrap();
    let s = scan(&root.join("repo"), &[], Some(&root));
    let loaded: u64 = s.files.iter().map(|f| f.loaded_bytes).sum();
    let expected: u64 = s
        .files
        .iter()
        .filter(|f| f.state.is_loaded())
        .map(|f| f.bytes)
        .sum();
    assert_eq!(loaded, expected);
    assert_eq!(s.loaded_est_tokens, loaded.div_ceil(4));
    // Excluded by claudeMdExcludes, path-scoped rule, nested file: present, zero loaded bytes.
    for suffix in ["rules/always.md", "rules/api.md", "pkg/CLAUDE.md"] {
        let f = s.files.iter().find(|f| f.path.ends_with(suffix)).unwrap();
        assert_eq!(f.loaded_bytes, 0, "{suffix}");
        assert!(f.bytes > 0);
    }
    assert_eq!(state(&s, "rules/always.md"), LoadState::NotLoaded);
    assert_eq!(state(&s, "pkg/sub/CLAUDE.md"), LoadState::NotLoaded);
}

#[test]
fn findings_name_file_rule_measure_certainty_and_check() {
    let root = fs::canonicalize(fixtures().join("claude")).unwrap();
    let s = scan(&root.join("repo"), &[], Some(&root));
    let broken: Vec<_> = s
        .findings
        .iter()
        .filter(|f| f.rule == "broken-import")
        .collect();
    assert_eq!(broken.len(), 2);
    assert!(broken.iter().any(|f| f.file.ends_with("docs/gone.md")));
    let dup = s
        .findings
        .iter()
        .find(|f| f.rule == "duplicate-instructions")
        .unwrap();
    assert!(dup.measure.contains("guide-copy.md") && dup.measure.contains("guide.md"));
    for f in &s.findings {
        assert!(!f.file.is_empty() && !f.measure.is_empty() && !f.check.is_empty());
        assert!(["high", "medium", "low"].contains(&f.certainty));
    }
}

#[test]
fn a_worktree_loads_the_files_above_it_and_same_names_stay_apart() {
    let root = tree("worktree");
    write(&root.join("repo/CLAUDE.md"), "repo\n");
    write(&root.join("repo/app/CLAUDE.md"), "app one\n");
    write(&root.join("repo/.claude/worktrees/feat/CLAUDE.md"), "wt\n");
    write(
        &root.join("repo/.claude/worktrees/feat/app/CLAUDE.md"),
        "app two\n",
    );
    let s = scan(&root.join("repo/.claude/worktrees/feat"), &[], Some(&root));
    // The main checkout's file is an ancestor by path and loads.
    assert_eq!(state(&s, "repo/CLAUDE.md"), LoadState::Expected);
    // The worktree's own `app` is a different file from the main checkout's.
    let apps: Vec<_> = s
        .files
        .iter()
        .filter(|f| f.path.ends_with("app/CLAUDE.md"))
        .collect();
    assert_eq!(apps.len(), 1);
    assert!(apps[0].path.contains("worktrees/feat/app"));
    // From the main checkout the worktrees are not merged in, only pointed out.
    let main = scan(&root.join("repo"), &[], Some(&root));
    assert!(!main.files.iter().any(|f| f.path.contains("worktrees")));
    assert!(main.notes.iter().any(|n| n.contains("worktree")));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unreadable_missing_and_deep_imports_stay_visible() {
    let root = tree("imports");
    write(&root.join("CLAUDE.md"), "@a1.md @missing.md\n");
    for i in 1..=5 {
        write(&root.join(format!("a{i}.md")), &format!("@a{}.md\n", i + 1));
    }
    write(&root.join("a6.md"), "end\n");
    let s = scan(&root, &[], Some(&root));
    assert_eq!(state(&s, "missing.md"), LoadState::Missing);
    // Four hops load; the fifth is listed as not loaded.
    assert_eq!(state(&s, "a4.md"), LoadState::Expected);
    assert_eq!(state(&s, "a5.md"), LoadState::NotLoaded);
    let _ = fs::remove_dir_all(root);
}
