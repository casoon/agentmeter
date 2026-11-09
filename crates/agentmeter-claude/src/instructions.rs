//! `--instructions`: the instruction files Claude Code loads for a directory, found by its
//! documented loading rules without starting Claude Code.
//!
//! Rules followed (Claude Code memory documentation, as of 2026-11):
//! - `CLAUDE.md`, `.claude/CLAUDE.md` and `CLAUDE.local.md` in the directory and every directory
//!   above it load at launch; the same files in subdirectories load once Claude touches a file
//!   there (on demand).
//! - `AGENTS.md` is read only when no `CLAUDE.md`-style file exists in the directory or above.
//! - `~/.claude/CLAUDE.md` and `~/.claude/rules/` load always; `.claude/rules/*.md` of the
//!   project load at launch unless the file has `paths:` frontmatter (then on demand).
//! - `@path` imports in a loaded file load with it, up to four hops deep.
//! - `claudeMdExcludes` (settings) skips files by absolute-path glob; managed files cannot be
//!   excluded.
//!
//! Whether a file is really loaded is only known from `--probe`; without it the state is
//! `expected`.

use agentmeter_core::instructions::{InstructionFile, InstructionScan, LoadState};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const RULES_VERSION: &str = "Claude Code memory docs, 2026-11";
const MAX_IMPORT_HOPS: usize = 4;
const MAX_WALK_DIRS: usize = 5_000;
const MAX_WALK_DEPTH: usize = 6;
const SKIPPED_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "vendor"];
const MANAGED: &[&str] = &[
    "/Library/Application Support/ClaudeCode/CLAUDE.md",
    "/etc/claude-code/CLAUDE.md",
];

/// Scans `cwd`. `ceiling` stops the walk upwards after that directory (tests); `None` walks to
/// the root as Claude Code does.
pub fn scan(cwd: &Path, config_dirs: &[PathBuf], ceiling: Option<&Path>) -> InstructionScan {
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let excludes = excludes(&cwd, config_dirs);
    let mut ctx = Ctx {
        files: Vec::new(),
        seen: HashSet::new(),
        excludes,
    };
    let mut notes = Vec::new();

    for path in MANAGED {
        ctx.add(Path::new(path), "managed", LoadState::Expected, false);
    }
    for dir in config_dirs {
        ctx.add(&dir.join("CLAUDE.md"), "user", LoadState::Expected, false);
        for rule in markdown_files(&dir.join("rules")) {
            ctx.add_rule(&rule, "user", false);
        }
    }

    let mut had_claude_md = false;
    let mut agents = Vec::new();
    let mut dir = Some(cwd.as_path());
    while let Some(d) = dir {
        let scope = if d == cwd { "project" } else { "ancestor" };
        for name in ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"] {
            let path = d.join(name);
            if path.is_file() {
                had_claude_md = true;
                ctx.add(&path, scope, LoadState::Expected, true);
            }
        }
        let agents_md = d.join("AGENTS.md");
        if agents_md.is_file() {
            agents.push((agents_md, scope));
        }
        if ceiling == Some(d) {
            break;
        }
        dir = d.parent();
    }
    for (path, scope) in agents {
        if had_claude_md {
            let mut file = ctx.add(&path, scope, LoadState::NotLoaded, false);
            if let Some(f) = file.as_mut() {
                f.note = Some(
                    "AGENTS.md is read only when no CLAUDE.md exists in the directory or above"
                        .to_owned(),
                );
            }
        } else {
            ctx.add(&path, scope, LoadState::Expected, true);
        }
    }
    for rule in markdown_files(&cwd.join(".claude/rules")) {
        ctx.add_rule(&rule, "rules", true);
    }
    let mut budget = MAX_WALK_DIRS;
    let mut nested = Vec::new();
    walk(&cwd, 0, &mut budget, &mut nested);
    if budget == 0 {
        notes.push(format!(
            "subdirectory scan stopped after {MAX_WALK_DIRS} directories"
        ));
    }
    for path in nested {
        ctx.add(&path, "nested", LoadState::OnDemand, true);
    }
    let worktrees = cwd.join(".claude/worktrees");
    if let Ok(entries) = fs::read_dir(&worktrees) {
        let n = entries.filter_map(Result::ok).count();
        if n > 0 {
            notes.push(format!(
                "{n} worktree(s) under .claude/worktrees are not scanned here; scan a worktree's \
                 own path to see its files (it also loads the files of the directories above it)"
            ));
        }
    }
    notes.push(
        "Not covered: files Claude loads for other directories it touches during a session, \
         and managed settings beyond the two documented CLAUDE.md paths."
            .to_owned(),
    );
    InstructionScan::new(
        "claude",
        RULES_VERSION,
        cwd.display().to_string(),
        ctx.files,
        notes,
    )
    .finish()
}

struct Ctx {
    files: Vec<InstructionFile>,
    seen: HashSet<PathBuf>,
    excludes: Vec<String>,
}

impl Ctx {
    /// Adds a file if it exists and was not listed; follows its imports when `imports` is set
    /// and the file counts as loaded (or on demand).
    fn add(
        &mut self,
        path: &Path,
        scope: &'static str,
        state: LoadState,
        imports: bool,
    ) -> Option<&mut InstructionFile> {
        if !path.is_file() || !self.seen.insert(path.to_path_buf()) {
            return None;
        }
        let key = path.display().to_string();
        let mut state = state;
        let mut note = None;
        if scope != "managed"
            && scope != "user"
            && self.excludes.iter().any(|g| glob_match(g, &key))
        {
            state = LoadState::NotLoaded;
            note = Some("excluded by claudeMdExcludes".to_owned());
        }
        let mut file = InstructionFile::new(key, scope, state);
        let content = match fs::read(path) {
            Ok(content) => content,
            Err(e) => {
                file.state = LoadState::Unreadable;
                file.note = Some(format!("could not be read: {e}"));
                self.files.push(file);
                return self.files.last_mut();
            }
        };
        file = file.with_content(&content);
        file.note = note;
        let follow = imports && matches!(file.state, LoadState::Expected | LoadState::OnDemand);
        let (path_s, state) = (file.path.clone(), file.state);
        self.files.push(file);
        if follow {
            self.follow_imports(path, &path_s, &content, state, 1);
        }
        self.files.iter_mut().find(|f| f.path == path_s)
    }

    /// A rule file: `paths:` frontmatter makes it conditional.
    fn add_rule(&mut self, path: &Path, scope: &'static str, imports: bool) {
        let conditional = fs::read(path)
            .map(|c| has_paths_frontmatter(&String::from_utf8_lossy(&c)))
            .unwrap_or(false);
        let state = if conditional {
            LoadState::OnDemand
        } else {
            LoadState::Expected
        };
        if let Some(file) = self.add(path, scope, state, imports)
            && conditional
            && file.state == LoadState::OnDemand
        {
            file.note = Some("path-scoped rule: loads when a matching file is touched".into());
        }
    }

    fn follow_imports(
        &mut self,
        from: &Path,
        from_s: &str,
        content: &[u8],
        state: LoadState,
        hop: usize,
    ) {
        for target in imports_of(&String::from_utf8_lossy(content)) {
            let resolved = resolve(from, &target);
            let key = resolved.display().to_string();
            if hop > MAX_IMPORT_HOPS {
                if resolved.is_file() && self.seen.insert(resolved.clone()) {
                    let mut f = InstructionFile::new(key, "import", LoadState::NotLoaded);
                    f.via = Some(from_s.to_owned());
                    f.note = Some(format!("beyond {MAX_IMPORT_HOPS} import hops"));
                    self.files.push(f);
                }
                continue;
            }
            if !self.seen.insert(resolved.clone()) {
                continue;
            }
            let mut file = InstructionFile::new(key.clone(), "import", state);
            file.via = Some(from_s.to_owned());
            match fs::read(&resolved) {
                Ok(content) if resolved.is_file() => {
                    let file = file.with_content(&content);
                    self.files.push(file);
                    self.follow_imports(&resolved, &key, &content, state, hop + 1);
                }
                Ok(_) | Err(_) if !resolved.exists() => {
                    file.state = LoadState::Missing;
                    self.files.push(file);
                }
                _ => {
                    file.state = LoadState::Unreadable;
                    file.note = Some("could not be read".to_owned());
                    self.files.push(file);
                }
            }
        }
    }
}

/// `@path` imports of a Markdown text: not in code fences or inline code, and only tokens that
/// look like a file path.
fn imports_of(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        // Even segments of a split at backticks are outside inline code.
        for (i, segment) in line.split('`').enumerate() {
            if i % 2 == 1 {
                continue;
            }
            for token in segment.split_whitespace() {
                let token = token.trim_start_matches(['(', '-', '*']);
                let Some(path) = token.strip_prefix('@') else {
                    continue;
                };
                let path = path.trim_end_matches(['.', ',', ';', ':', ')', '!', '?']);
                if looks_like_path(path) && !found.iter().any(|p| p == path) {
                    found.push(path.to_owned());
                }
            }
        }
    }
    found
}

fn looks_like_path(token: &str) -> bool {
    if token.is_empty() || token.contains(['@', ':']) {
        return false;
    }
    if ["~/", "/", "./", "../"]
        .iter()
        .any(|p| token.starts_with(p))
    {
        return true;
    }
    let last = token.rsplit('/').next().unwrap_or(token);
    match last.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

fn resolve(from: &Path, target: &str) -> PathBuf {
    if let Some(rest) = target.strip_prefix("~/") {
        return std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(target));
    }
    let path = Path::new(target);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        from.parent().unwrap_or(Path::new("/")).join(path)
    };
    fs::canonicalize(&joined).unwrap_or(joined)
}

fn has_paths_frontmatter(text: &str) -> bool {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return false;
    }
    lines
        .take_while(|l| l.trim() != "---")
        .any(|l| l.trim_start().starts_with("paths:"))
}

/// `*.md` files below a directory, sorted; empty when it does not exist.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut visited = 0;
    while let Some(d) = stack.pop() {
        visited += 1;
        if visited > MAX_WALK_DIRS {
            break;
        }
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// `CLAUDE.md` and `CLAUDE.local.md` in subdirectories; hidden and build directories are left
/// out, so are `.claude/worktrees` (hidden).
fn walk(dir: &Path, depth: usize, budget: &mut usize, found: &mut Vec<PathBuf>) {
    if depth >= MAX_WALK_DEPTH || *budget == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            !name.starts_with('.') && !SKIPPED_DIRS.contains(&name)
        })
        .collect();
    dirs.sort();
    for sub in dirs {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        for name in ["CLAUDE.md", "CLAUDE.local.md"] {
            let path = sub.join(name);
            if path.is_file() {
                found.push(path);
            }
        }
        walk(&sub, depth + 1, budget, found);
    }
}

/// `claudeMdExcludes` of the project's and the user's settings.
fn excludes(cwd: &Path, config_dirs: &[PathBuf]) -> Vec<String> {
    let mut files = vec![
        cwd.join(".claude/settings.local.json"),
        cwd.join(".claude/settings.json"),
    ];
    files.extend(config_dirs.iter().map(|d| d.join("settings.json")));
    let mut out = Vec::new();
    for file in files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(list) = value["claudeMdExcludes"].as_array() {
            out.extend(list.iter().filter_map(|v| v.as_str().map(str::to_owned)));
        }
    }
    out
}

/// Glob over absolute paths: `**` matches anything, `*` anything but `/`, `?` one character
/// but `/`.
fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                (0..=t.len()).any(|i| go(rest, &t[i..]))
            }
            Some(b'*') => {
                let rest = &p[1..];
                (0..=t.len())
                    .take_while(|&i| i == 0 || t[i - 1] != b'/')
                    .any(|i| go(rest, &t[i..]))
            }
            Some(b'?') => t.first().is_some_and(|c| *c != b'/') && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match(
            "**/monorepo/CLAUDE.md",
            "/home/u/monorepo/CLAUDE.md"
        ));
        assert!(glob_match("/a/.claude/rules/**", "/a/.claude/rules/x/y.md"));
        assert!(!glob_match("/a/*.md", "/a/b/c.md"));
        assert!(glob_match("/a/*.md", "/a/c.md"));
    }

    #[test]
    fn imports_skip_code_and_non_paths() {
        let text = "See @docs/a.md and @~/x.md, mail me@x.org, @scope/pkg.\n\
                    `@inline.md`\n```\n@fenced.md\n```\n- @b.md";
        assert_eq!(imports_of(text), ["docs/a.md", "~/x.md", "b.md"]);
    }

    #[test]
    fn frontmatter() {
        assert!(has_paths_frontmatter("---\npaths:\n  - a\n---\nx"));
        assert!(!has_paths_frontmatter("# x\npaths: y"));
    }
}
