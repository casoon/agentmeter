//! The Codex instruction scan against temporary trees.

use agentmeter_codex::instructions::scan;
use agentmeter_core::instructions::{InstructionScan, LoadState};
use std::fs;
use std::path::{Path, PathBuf};

fn tree(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("agentmeter-cxinstr-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn find<'a>(
    s: &'a InstructionScan,
    suffix: &str,
) -> &'a agentmeter_core::instructions::InstructionFile {
    s.files
        .iter()
        .find(|f| f.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("{suffix} not listed"))
}

#[test]
fn root_to_directory_with_override_and_nothing_from_below() {
    let root = tree("chain");
    let home = root.join("home");
    write(&home.join("AGENTS.md"), "global\n");
    write(&root.join("repo/.git/HEAD"), "ref\n");
    write(&root.join("repo/AGENTS.md"), "root\n");
    write(&root.join("repo/svc/AGENTS.override.md"), "override\n");
    write(&root.join("repo/svc/AGENTS.md"), "shadowed\n");
    write(&root.join("repo/svc/deep/AGENTS.md"), "below\n");
    write(&root.join("AGENTS.md"), "above the root\n");
    let s = scan(&root.join("repo/svc"), &[home], Some(&root));
    assert_eq!(find(&s, "home/AGENTS.md").scope, "user");
    assert_eq!(find(&s, "repo/AGENTS.md").state, LoadState::Expected);
    assert_eq!(find(&s, "AGENTS.override.md").state, LoadState::Expected);
    assert_eq!(find(&s, "svc/AGENTS.md").state, LoadState::NotLoaded);
    // Not past the project root, not below the directory.
    assert!(!s.files.iter().any(|f| f.path.ends_with("deep/AGENTS.md")));
    assert!(
        !s.files
            .iter()
            .any(|f| f.path == root.join("AGENTS.md").display().to_string())
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_byte_budget_cuts_files_and_says_so() {
    let root = tree("budget");
    let home = root.join("home");
    write(&home.join("config.toml"), "project_doc_max_bytes = 30\n");
    write(&root.join("repo/.git"), "gitdir: elsewhere\n");
    write(&root.join("repo/AGENTS.md"), &"a".repeat(20));
    write(&root.join("repo/svc/AGENTS.md"), &"b".repeat(20));
    write(&root.join("repo/svc/x/AGENTS.md"), &"c".repeat(20));
    let s = scan(&root.join("repo/svc/x"), &[home], Some(&root));
    assert_eq!(find(&s, "repo/AGENTS.md").loaded_bytes, 20);
    let cut = find(&s, "svc/AGENTS.md");
    assert_eq!((cut.state, cut.loaded_bytes), (LoadState::Expected, 10));
    assert_eq!(find(&s, "x/AGENTS.md").state, LoadState::NotLoaded);
    assert_eq!(s.loaded_est_tokens, 30u64.div_ceil(4));
    assert!(
        s.findings
            .iter()
            .any(|f| f.rule == "load-limit" && f.certainty == "high")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn without_a_git_marker_only_the_directory_is_searched() {
    let root = tree("nogit");
    write(&root.join("AGENTS.md"), "above\n");
    write(&root.join("proj/AGENTS.md"), "here\n");
    let s = scan(&root.join("proj"), &[], Some(&root));
    assert_eq!(s.files.len(), 1);
    assert!(s.notes.iter().any(|n| n.contains(".git")));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_directory_named_like_the_file_is_skipped() {
    let root = tree("unreadable");
    write(&root.join("proj/AGENTS.md"), "x\n");
    // A directory named like the file is not a regular file and is skipped, not misreported.
    fs::create_dir_all(root.join("proj/svc/AGENTS.md")).unwrap();
    let s = scan(&root.join("proj/svc"), &[], Some(&root));
    assert!(s.files.iter().all(|f| !f.path.ends_with("svc/AGENTS.md")));
    let _ = fs::remove_dir_all(root);
}
