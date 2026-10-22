//! Finding skills: folders holding a `SKILL.md` in each agent's skill roots.

use std::path::{Path, PathBuf};

use crate::{Agent, Plugin, Roots, Session};

/// A skill folder, wherever it is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Stable and root-qualified: `agents:user:<folder>`, `claude:project:<path>:<folder>`.
    pub id: String,
    /// Its `SKILL.md` frontmatter's name, else its folder's.
    pub name: String,
    pub description: String,
    /// Its folder, symlinks resolved: one skill linked into several roots is one entry.
    pub dir: PathBuf,
    /// Every root it is found in, in [`SkillRoot`]'s order.
    pub roots: Vec<SkillRoot>,
    /// Each folder it is found at, as found (a link into a root is not resolved), with its
    /// root: Codex knows a skill by the path it found it at.
    pub entries: Vec<(SkillRoot, PathBuf)>,
}

impl Skill {
    pub fn skill_md(&self) -> PathBuf {
        self.dir.join("SKILL.md")
    }

    /// The plugin it comes with when it is found in plugins only (its first), whose agent
    /// shows it as `<plugin>:<name>`.
    pub fn plugin(&self) -> Option<&Plugin> {
        let mut plugins = self.roots.iter().map(|root| match root {
            SkillRoot::Plugin(plugin) => Some(plugin),
            _ => None,
        });
        let first = plugins.next()??;
        plugins.all(|plugin| plugin.is_some()).then_some(first)
    }
}

/// A folder an agent finds skills in by itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SkillRoot {
    /// `~/.agents/skills` (Codex).
    AgentsUser,
    /// `~/.claude/skills` (Claude Code; `CLAUDE_CONFIG_DIR/skills` when that is set).
    ClaudeUser,
    /// `$CODEX_HOME/skills` (Codex), its `.system` folder left out.
    CodexUser,
    /// `~/.claude/skills/synced/<org>_<account>`: the skills Claude Code syncs from a Claude
    /// account (it shows them as `synced-skill:<name>`).
    ClaudeSynced(String),
    /// A project's `.claude/skills` (Claude Code).
    ClaudeProject(PathBuf),
    /// A project's `.agents/skills` (Codex, in a checkout of the project).
    AgentsProject(PathBuf),
    /// A plugin its agent loads, which shows the skill as `<plugin>:<name>`.
    Plugin(Plugin),
}

impl SkillRoot {
    /// Whether `session`'s agent finds the skills in this root by itself.
    pub fn native(&self, session: &Session<'_>) -> bool {
        match (self, session.agent) {
            (Self::ClaudeUser | Self::ClaudeSynced(_), Agent::Claude)
            | (Self::AgentsUser | Self::CodexUser, Agent::Codex) => true,
            (Self::ClaudeProject(project), Agent::Claude) => {
                Some(project.as_path()) == session.project
            }
            (Self::AgentsProject(project), Agent::Codex) => {
                Some(project.as_path()) == session.project && session.in_checkout
            }
            (Self::Plugin(plugin), agent) => {
                plugin.agent == agent && plugin.counts_in(session.project)
            }
            _ => false,
        }
    }

    fn prefix(&self) -> String {
        match self {
            Self::AgentsUser => "agents:user".into(),
            Self::ClaudeUser => "claude:user".into(),
            Self::CodexUser => "codex:user".into(),
            Self::ClaudeSynced(account) => format!("claude:synced:{account}"),
            Self::ClaudeProject(project) => format!("claude:project:{}", project.display()),
            Self::AgentsProject(project) => format!("agents:project:{}", project.display()),
            Self::Plugin(plugin) => plugin.prefix(),
        }
    }
}

/// The user's skills.
pub(crate) fn add_user(skills: &mut Vec<Skill>, roots: &Roots) {
    scan(skills, &roots.agents_skills, SkillRoot::AgentsUser);
    scan(
        skills,
        &roots.claude_dir.join("skills"),
        SkillRoot::ClaudeUser,
    );
    scan(
        skills,
        &roots.codex_home.join("skills"),
        SkillRoot::CodexUser,
    );
    let synced = roots.claude_dir.join("skills").join("synced");
    if let Ok(accounts) = std::fs::read_dir(&synced) {
        let mut accounts: Vec<_> = accounts.flatten().collect();
        accounts.sort_by_key(|entry| entry.file_name());
        for account in accounts {
            let name = account.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') {
                scan(skills, &account.path(), SkillRoot::ClaudeSynced(name));
            }
        }
    }
}

/// A project's skills.
pub(crate) fn add_project(skills: &mut Vec<Skill>, project: &Path) {
    scan(
        skills,
        &project.join(".claude").join("skills"),
        SkillRoot::ClaudeProject(project.to_owned()),
    );
    scan(
        skills,
        &project.join(".agents").join("skills"),
        SkillRoot::AgentsProject(project.to_owned()),
    );
}

/// The skills of `plugin`, each with its folder's path in the plugin in its id.
pub(crate) fn add_plugin(skills: &mut Vec<Skill>, plugin: &Plugin) {
    let root = SkillRoot::Plugin(plugin.clone());
    for dir in plugin.skill_dirs() {
        let folder_name = dir.strip_prefix(&plugin.root).map_or_else(
            |_| dir.display().to_string(),
            |relative| relative.to_string_lossy().into_owned(),
        );
        let folder_name = if folder_name.is_empty() {
            plugin.name.clone()
        } else {
            folder_name
        };
        add(skills, &dir, &folder_name, &root);
    }
}

/// Adds `skill` to `skills`, or merges its roots and folders into the one it is: the same folder,
/// or a copy of it file for file.
pub(crate) fn merge(skills: &mut Vec<Skill>, skill: Skill) {
    let known = skills.iter_mut().find(|known| {
        known.dir == skill.dir
            || (known.name == skill.name && same_contents(&known.dir, &skill.dir))
    });
    let Some(known) = known else {
        skills.push(skill);
        return;
    };
    for root in skill.roots {
        if !known.roots.contains(&root) {
            known.roots.push(root);
        }
    }
    known.entries.extend(skill.entries);
}

/// Every skill folder directly in `folder` (hidden ones, such as Codex's `.system`, left out).
fn scan(skills: &mut Vec<Skill>, folder: &Path, root: SkillRoot) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let folder_name = entry.file_name().to_string_lossy().into_owned();
        if !folder_name.starts_with('.') {
            add(skills, &entry.path(), &folder_name, &root);
        }
    }
}

/// The skill at `path` (found in `root` as `folder_name`), if it is one.
fn add(skills: &mut Vec<Skill>, path: &Path, folder_name: &str, root: &SkillRoot) {
    let Ok(dir) = std::fs::canonicalize(path) else {
        return;
    };
    if let Some(known) = skills.iter_mut().find(|skill| skill.dir == dir) {
        if !known.roots.contains(root) {
            known.roots.push(root.clone());
        }
        known.entries.push((root.clone(), path.to_owned()));
        return;
    }
    let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else {
        return;
    };
    let (name, description) = frontmatter(&text);
    let name = name.unwrap_or_else(|| folder_name.to_owned());
    // A copy (not a link) of a skill already found, file for file: the same skill.
    if let Some(known) = skills
        .iter_mut()
        .find(|skill| skill.name == name && same_contents(&skill.dir, &dir))
    {
        if !known.roots.contains(root) {
            known.roots.push(root.clone());
        }
        known.entries.push((root.clone(), path.to_owned()));
        return;
    }
    skills.push(Skill {
        id: format!("{}:{folder_name}", root.prefix()),
        name,
        description: description.unwrap_or_default(),
        dir,
        roots: vec![root.clone()],
        entries: vec![(root.clone(), path.to_owned())],
    });
}

/// Takes `plugin`'s skills out of `skills`: a skill found nowhere else goes.
pub(crate) fn remove_plugin(skills: &mut Vec<Skill>, plugin: &Plugin) {
    let root = SkillRoot::Plugin(plugin.clone());
    for skill in skills.iter_mut() {
        skill.roots.retain(|other| *other != root);
        skill.entries.retain(|(other, _)| *other != root);
    }
    skills.retain(|skill| !skill.roots.is_empty());
}

/// Whether two folders hold the same files with the same bytes (hidden ones included).
fn same_contents(a: &Path, b: &Path) -> bool {
    fn files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if std::fs::metadata(&path)?.is_dir() {
                files(root, &path, out)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_owned());
            }
        }
        Ok(())
    }
    let list = |root: &Path| {
        let mut out = Vec::new();
        files(root, root, &mut out).ok()?;
        out.sort();
        Some(out)
    };
    let (Some(left), Some(right)) = (list(a), list(b)) else {
        return false;
    };
    left == right
        && left.iter().all(|file| {
            matches!(
                (std::fs::read(a.join(file)), std::fs::read(b.join(file))),
                (Ok(x), Ok(y)) if x == y
            )
        })
}

/// `name` and `description` from a `SKILL.md`'s YAML frontmatter.
fn frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (None, None);
    };
    let end = rest.find("\n---").map_or(rest.len(), |end| end + 1);
    let Ok(docs) = yaml_rust2::YamlLoader::load_from_str(&rest[..end]) else {
        return (None, None);
    };
    let Some(doc) = docs.first() else {
        return (None, None);
    };
    let field = |key: &str| {
        doc[key]
            .as_str()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    (field("name"), field("description"))
}
