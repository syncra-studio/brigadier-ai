//! Data-dir Library content helpers and read-only access to migrated packages.
use crate::adapter::{CLI_TIMEOUT, output_within};
use crate::item::SkillSource;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
pub const MARKETPLACE: &str = "brigadier";
/// One plugin in the Library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Its folder.
    pub dir: PathBuf,
}

impl Entry {
    /// What the agents' plugin commands call it.
    pub fn id(&self) -> String {
        plugin_id(&self.name)
    }
}

/// `<name>@brigadier`.
pub fn plugin_id(name: &str) -> String {
    format!("{name}@{MARKETPLACE}")
}

/// Where a Library item fetched from Git or skills.sh came from, and the commit it was packed
/// from: what "Update available" compares with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Origin {
    pub source: SkillSource,
    pub commit: String,
}

pub struct Library {
    root: PathBuf,
}
impl Library {
    /// The Library in `home`'s `.brigadier/library`.
    pub fn in_home(home: &Path, brigadier: PathBuf) -> Self {
        Self::at(home.join(".brigadier").join("library"), brigadier)
    }

    pub fn at(root: PathBuf, _brigadier: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn marketplace_file(&self) -> PathBuf {
        self.root.join(".claude-plugin").join("marketplace.json")
    }

    fn plugin_dir(&self, name: &str) -> PathBuf {
        self.root.join("plugins").join(name)
    }

    /// Its plugins, as its marketplace lists them.
    pub fn entries(&self) -> Vec<Entry> {
        let Ok(text) = std::fs::read_to_string(self.marketplace_file()) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        value
            .get("plugins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|plugin| {
                let name = plugin.get("name")?.as_str()?.to_owned();
                Some(Entry {
                    dir: self.plugin_dir(&name),
                    version: plugin
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    description: plugin
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    name,
                })
            })
            .collect()
    }
}
pub fn fetch_skill(source: &SkillSource, scratch: &Path) -> Result<PathBuf, String> {
    let (url, rev, subdir, wanted) = match source {
        SkillSource::Folder(dir) => return Ok(dir.clone()),
        SkillSource::Git { url, rev, subdir } => (url.clone(), rev.clone(), subdir.clone(), None),
        SkillSource::SkillsSh(id) => {
            let mut parts = id.split('/');
            let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
                return Err(format!("\"{id}\" isn't a skills.sh name."));
            };
            (
                format!("https://github.com/{owner}/{repo}"),
                None,
                None,
                parts.next().map(str::to_owned),
            )
        }
    };
    let checkout = scratch.join("checkout");
    let mut git = Command::new("git");
    git.args(["clone", "--quiet", "--depth", "1"]);
    if let Some(rev) = &rev {
        git.args(["--branch", rev]);
    }
    let out = output_within(
        git.arg(&url).arg(&checkout).env("GIT_TERMINAL_PROMPT", "0"),
        CLI_TIMEOUT,
    )
    .map_err(|err| format!("Couldn't run git: {err}."))?;
    if !out.status.success() {
        return Err(format!("Couldn't download {url}."));
    }
    let base = match &subdir {
        Some(subdir) => checkout.join(subdir),
        None => checkout.clone(),
    };
    if base.join("SKILL.md").is_file() && wanted.is_none() {
        return Ok(base);
    }
    // The skill by name: a folder holding `SKILL.md` whose name (or folder) matches.
    let mut found = Vec::new();
    skill_folders(&base, 0, &mut found);
    let pick = |dir: &&PathBuf| match &wanted {
        None => true,
        Some(wanted) => {
            let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
            crate::skills::frontmatter(&text).0.as_deref() == Some(wanted)
                || dir.file_name().is_some_and(|name| name == wanted.as_str())
        }
    };
    let matching: Vec<&PathBuf> = found.iter().filter(pick).collect();
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("No skill found in {url}.")),
        _ => Err(format!("{url} has several skills: name the one you want.")),
    }
}

/// The repository `source` is fetched from, and the branch or tag (`None`: its default).
fn remote(source: &SkillSource) -> Option<(String, Option<String>)> {
    match source {
        SkillSource::Folder(_) => None,
        SkillSource::Git { url, rev, .. } => Some((url.clone(), rev.clone())),
        SkillSource::SkillsSh(id) => {
            let mut parts = id.split('/');
            Some((
                format!("https://github.com/{}/{}", parts.next()?, parts.next()?),
                None,
            ))
        }
    }
}

/// The name the skill at `dir` gives itself in its `SKILL.md`.
pub fn skill_name(dir: &Path) -> Option<String> {
    crate::skills::frontmatter(&std::fs::read_to_string(dir.join("SKILL.md")).ok()?).0
}

/// The commit [`fetch_skill`] fetched into `scratch`, for a Git or skills.sh source.
pub fn fetched_commit(scratch: &Path) -> Option<String> {
    let out = output_within(
        Command::new("git")
            .arg("-C")
            .arg(scratch.join("checkout"))
            .args(["rev-parse", "HEAD"]),
        CLI_TIMEOUT,
    )
    .ok()?;
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !commit.is_empty()).then_some(commit)
}

/// The commit `source`'s repository is at now (its branch or tag, else its default branch).
pub fn latest_commit(source: &SkillSource) -> Result<String, String> {
    let (url, rev) = remote(source).ok_or("A folder has no newer version.")?;
    let out = output_within(
        Command::new("git")
            .args(["ls-remote", &url, rev.as_deref().unwrap_or("HEAD")])
            .env("GIT_TERMINAL_PROMPT", "0"),
        CLI_TIMEOUT,
    )
    .map_err(|err| format!("Couldn't run git: {err}."))?;
    if !out.status.success() {
        return Err(format!("Couldn't reach {url}."));
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "{url} has no {}.",
                rev.as_deref().unwrap_or("default branch")
            )
        })
}

/// The folders under `dir` (a few levels down) that hold `SKILL.md`.
fn skill_folders(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    if dir.join("SKILL.md").is_file() {
        out.push(dir.to_owned());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !path.ends_with(".git"))
        .collect();
    children.sort();
    for child in children {
        skill_folders(&child, depth + 1, out);
    }
}

/// `${CLAUDE_PLUGIN_ROOT}` and `${PLUGIN_ROOT}` in `text` as `root`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The SHA-256 of `text`, in hex: what remembers a value without keeping it.
pub fn hash_text(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The hash of every file in `dir` (names and bytes): what tells a skill folder changed.
pub fn hash_dir(dir: &Path) -> std::io::Result<String> {
    hash_tree(dir)
}

/// A plugin's or server's name: letters, digits, `-` and `_`, not starting with `.`.
fn hash_tree(dir: &Path) -> std::io::Result<String> {
    fn walk(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, files)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_owned());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for file in files {
        if file == Path::new(".brigadier.json") || file.starts_with(".claude-plugin") {
            continue;
        }
        if file.starts_with(".codex-plugin") {
            continue;
        }
        let bytes = std::fs::read(dir.join(&file))?;
        let name = file.to_string_lossy();
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
#[path = "library/tests.rs"]
mod tests;
