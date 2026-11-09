//! `--skills`: the installed Claude Code skills found on disk, for the size of their entries in
//! the skill list. Only the header of each `SKILL.md` is read, and only for the name and the
//! length of the description; no content is kept.
//!
//! Rules followed (Claude Code skills documentation, as of 2026-11, checked against Claude Code
//! 2.1.316):
//! - Personal skills are `~/.claude/skills/<name>/SKILL.md`, project skills
//!   `.claude/skills/<name>/SKILL.md`, plugin skills `<plugin>/skills/<name>/SKILL.md`, called
//!   `plugin:name`.
//! - The name is the frontmatter `name`, else the directory name; the directory name also starts
//!   the skill.
//! - Name and description of a model-invocable skill are in the context at session start; the
//!   `description` and `when_to_use` text of one entry is cut at 1,536 characters.
//! - `disable-model-invocation: true` keeps the entry out of the list.
//! - Without a `description` the first non-empty line of the text is used.
//!
//! Not covered: enterprise skills, nested `.claude/skills` below the working directory, added
//! directories, skills synced from claude.ai, built-in skills, `skillOverrides` and plugin
//! enablement outside the user settings. `--probe` shows what Claude Code lists.

use agentmeter_core::skills::{InstalledSkill, LISTING_CAP_CHARS, ListState, SkillScan};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const RULES_VERSION: &str =
    "Claude Code skills docs, 2026-11 (checked with Claude Code 2.1.316)";
/// How much of a `SKILL.md` is read: the header is far shorter.
const HEAD_BYTES: u64 = 64 * 1024;

/// What the header of one `SKILL.md` says.
#[derive(Debug, PartialEq, Eq)]
pub struct Head {
    pub name: Option<String>,
    /// Bytes of `description` plus `when_to_use`, cut at the documented cap; 0 when there is none.
    pub text_bytes: u64,
    pub user_only: bool,
}

/// Reads the frontmatter. Keys at the top level only; a value may continue on indented lines
/// and may be a block scalar (`>`, `|`). Nothing is kept except lengths and the name.
pub fn parse_head(text: &str) -> Head {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    let mut fields: Vec<(String, Vec<String>)> = Vec::new();
    let has_frontmatter = text.lines().next().map(str::trim_end) == Some("---");
    if has_frontmatter {
        lines.next();
        for line in lines.by_ref() {
            if line.trim_end() == "---" {
                break;
            }
            if line.starts_with([' ', '\t']) {
                if let Some((_, parts)) = fields.last_mut() {
                    parts.push(line.trim().to_owned());
                }
            } else if let Some((key, value)) = line.split_once(':') {
                fields.push((key.trim().to_owned(), vec![value.trim().to_owned()]));
            }
        }
    }
    let value = |key: &str| -> Option<String> {
        let (_, parts) = fields.iter().find(|(k, _)| k == key)?;
        let mut parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        if parts[0].starts_with(['>', '|']) {
            parts.remove(0);
        }
        let joined = parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let unquoted = ['"', '\'']
            .iter()
            .find_map(|q| joined.strip_prefix(*q)?.strip_suffix(*q))
            .unwrap_or(&joined);
        Some(unquoted.to_owned())
    };
    let name = value("name").filter(|n| !n.is_empty());
    let description = value("description").unwrap_or_default();
    let when = value("when_to_use").unwrap_or_default();
    let mut listing = [description, when]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if listing.is_empty() {
        // The rest of the file after the header; without a header the whole file.
        listing = lines
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default()
            .to_owned();
    }
    let text_bytes = listing
        .char_indices()
        .nth(LISTING_CAP_CHARS)
        .map_or(listing.len(), |(i, _)| i) as u64;
    Head {
        name,
        text_bytes,
        user_only: value("disable-model-invocation").as_deref() == Some("true"),
    }
}

/// Scans the user skills and plugin skills of each configuration directory and the project
/// skills of each directory in `project_dirs` (working directories of the sessions in the
/// period).
pub fn scan(config_dirs: &[PathBuf], project_dirs: &[PathBuf]) -> SkillScan {
    let mut ctx = Ctx::default();
    for dir in config_dirs {
        ctx.add_dir(&dir.join("skills"), "user", None, true);
        ctx.plugins(dir);
    }
    let mut missing_projects = 0;
    for dir in project_dirs {
        if dir.is_dir() {
            ctx.add_dir(&dir.join(".claude/skills"), "project", None, false);
        } else {
            missing_projects += 1;
        }
    }
    let mut notes = ctx.notes;
    if missing_projects > 0 {
        notes.push(format!(
            "{missing_projects} working director{} of the sessions no longer exist; their project \
             skills are not scanned",
            if missing_projects == 1 { "y" } else { "ies" }
        ));
    }
    if ctx.clashes > 0 {
        notes.push(format!(
            "{} skill(s) share a name with one found earlier (user, then plugin, then project) and \
             are counted once",
            ctx.clashes
        ));
    }
    if ctx.disabled_plugins > 0 {
        notes.push(format!(
            "{} plugin(s) set to false in the user settings (enabledPlugins) are left out",
            ctx.disabled_plugins
        ));
    }
    notes.push(
        "Not scanned: enterprise skills, nested .claude/skills below a working directory, added \
         directories, skills synced from claude.ai, built-in skills, skillOverrides, and plugin \
         enablement set in project settings. Project skills come from the working directory of \
         each session only."
            .to_owned(),
    );
    SkillScan {
        rules_version: RULES_VERSION,
        entries: ctx.entries,
        notes,
    }
}

#[derive(Default)]
struct Ctx {
    entries: Vec<InstalledSkill>,
    seen_files: HashSet<PathBuf>,
    seen_names: HashSet<String>,
    notes: Vec<String>,
    clashes: u64,
    disabled_plugins: u64,
}

impl Ctx {
    /// One `skills/` directory. A missing directory is a note only where it is expected
    /// (`expected`), since most projects have none.
    fn add_dir(&mut self, dir: &Path, source: &'static str, prefix: Option<&str>, expected: bool) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if expected {
                    self.notes
                        .push(format!("no skills directory at {}", dir.display()));
                }
                return;
            }
            Err(e) => {
                self.notes
                    .push(format!("could not read {}: {e}", dir.display()));
                return;
            }
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for skill_dir in dirs {
            let file = skill_dir.join("SKILL.md");
            if !skill_dir.is_dir() || !file.is_file() {
                continue;
            }
            let canonical = fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
            if !self.seen_files.insert(canonical) {
                continue;
            }
            let dir_name = skill_dir
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let qualified = |name: &str| match prefix {
                Some(p) => format!("{p}:{name}"),
                None => name.to_owned(),
            };
            let head = read_head(&file);
            let (name, state, bytes) = match &head {
                Ok(h) => {
                    let name = qualified(h.name.as_deref().unwrap_or(&dir_name));
                    if h.user_only {
                        (name, ListState::UserOnly, 0)
                    } else {
                        let bytes = name.len() as u64 + h.text_bytes;
                        (name, ListState::Listed, bytes)
                    }
                }
                Err(e) => {
                    self.notes
                        .push(format!("could not read {}: {e}", file.display()));
                    (qualified(&dir_name), ListState::Unreadable, 0)
                }
            };
            if !self.seen_names.insert(name.clone()) {
                self.clashes += 1;
                continue;
            }
            let alias = qualified(&dir_name);
            self.entries.push(InstalledSkill {
                alias: (alias != name).then_some(alias),
                name,
                source,
                path: file.display().to_string(),
                state,
                bytes,
            });
        }
    }

    /// The plugins listed in `plugins/installed_plugins.json`, minus those the user settings
    /// switch off.
    fn plugins(&mut self, config: &Path) {
        let list = config.join("plugins/installed_plugins.json");
        let text = match fs::read_to_string(&list) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.notes
                    .push(format!("no plugin list at {}", list.display()));
                return;
            }
            Err(e) => {
                self.notes
                    .push(format!("could not read {}: {e}", list.display()));
                return;
            }
        };
        let Some(plugins) = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("plugins").and_then(|p| p.as_object().cloned()))
        else {
            self.notes.push(format!(
                "{} has no plugin list in a layout this reader knows",
                list.display()
            ));
            return;
        };
        let enabled: Option<serde_json::Map<String, serde_json::Value>> =
            fs::read_to_string(config.join("settings.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get("enabledPlugins").and_then(|e| e.as_object().cloned()));
        let mut keys: Vec<&String> = plugins.keys().collect();
        keys.sort();
        let mut paths = HashSet::new();
        for key in keys {
            if enabled
                .as_ref()
                .and_then(|e| e.get(key))
                .and_then(serde_json::Value::as_bool)
                == Some(false)
            {
                self.disabled_plugins += 1;
                continue;
            }
            let name = key.split('@').next().unwrap_or(key);
            let Some(install) = plugins[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i.get("installPath")?.as_str())
                .next_back()
            else {
                continue;
            };
            if !paths.insert(install.to_owned()) {
                continue;
            }
            let install = Path::new(install);
            if !install.is_dir() {
                self.notes.push(format!(
                    "plugin {name}: install path {} not found",
                    install.display()
                ));
                continue;
            }
            self.add_dir(&install.join("skills"), "plugin", Some(name), false);
        }
    }
}

fn read_head(path: &Path) -> std::io::Result<Head> {
    let mut bytes = Vec::new();
    File::open(path)?.take(HEAD_BYTES).read_to_end(&mut bytes)?;
    Ok(parse_head(&String::from_utf8_lossy(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_quoted_and_folded_values_give_their_lengths() {
        let h = parse_head(
            "---\nname: demo\ndescription: \"abcde\"\n---\nbody text that is not counted\n",
        );
        assert_eq!(h.name.as_deref(), Some("demo"));
        assert_eq!(h.text_bytes, 5);
        let h = parse_head(
            "---\nname: f\ndescription: >-\n  one two\n  three\nwhen_to_use: x\n---\nbody\n",
        );
        // "one two three" + " " + "x"
        assert_eq!(h.text_bytes, 15);
        assert!(!h.user_only);
    }

    #[test]
    fn the_listing_text_is_cut_at_the_documented_cap() {
        let long = "ä".repeat(2_000);
        let h = parse_head(&format!("---\ndescription: {long}\n---\n"));
        assert_eq!(h.text_bytes, (LISTING_CAP_CHARS * 2) as u64);
    }

    #[test]
    fn manual_only_skills_and_the_first_line_fallback() {
        let h = parse_head("---\nname: m\ndisable-model-invocation: true\ndescription: d\n---\n");
        assert!(h.user_only);
        let h = parse_head("---\nname: n\n---\n\nFirst line.\nSecond.\n");
        assert_eq!(h.text_bytes, "First line.".len() as u64);
        let h = parse_head("No header here\nmore\n");
        assert_eq!((h.name, h.text_bytes), (None, 14));
    }
}
