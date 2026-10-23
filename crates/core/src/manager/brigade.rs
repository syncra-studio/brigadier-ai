//! The `/brigade` skill (S15): how a session runs workers of its own through `brigadier`. It is
//! installed for every session and Chat, on both CLIs: a Claude session loads it as a plugin,
//! and Codex finds it in its user skills folder (`P/codex`). A user copy at
//! `~/.brigadier/brigade/SKILL.md` overrides the shipped one and survives updates; Settings
//! creates it from the shipped one, and resets it.

use std::path::{Path, PathBuf};

use super::SessionManager;
use crate::{Error, Result};

/// The skill as shipped.
pub(crate) const SKILL: &str = include_str!("brigade/SKILL.md");

/// The `/fusion` skill (S36), shipped beside it in the same plugin: one consult per CLI when
/// the user is uncertain, then one merged answer.
pub(crate) const FUSION: &str = include_str!("fusion/SKILL.md");

/// Whether the user's copy overrides the shipped skill, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrigadeSkill {
    pub custom: bool,
    pub path: PathBuf,
}

impl SessionManager {
    /// The user's copy of the skill.
    pub(super) fn brigade_override_path(&self) -> PathBuf {
        if cfg!(test) {
            return self.data_dir.join("brigade-user/SKILL.md");
        }
        std::env::home_dir()
            .unwrap_or_else(|| self.data_dir.clone())
            .join(".brigadier/brigade/SKILL.md")
    }

    /// The skill's plugin, written with the user's copy when there is one, else the shipped
    /// one; `None` when it can't be written.
    pub(super) fn brigade_plugin(&self) -> Option<PathBuf> {
        let text = user_copy(&self.brigade_override_path());
        match write_plugin(
            &self.data_dir.join("brigade"),
            text.as_deref().unwrap_or(SKILL),
        ) {
            Ok(plugin) => Some(plugin),
            Err(error) => {
                tracing::warn!(%error, "couldn't write the brigade skill");
                None
            }
        }
    }

    /// Whether the user's copy is in use.
    pub fn brigade_skill(&self) -> BrigadeSkill {
        let path = self.brigade_override_path();
        BrigadeSkill {
            custom: user_copy(&path).is_some(),
            path,
        }
    }

    /// Writes the user's copy as the shipped skill, to edit, unless one is there: sessions
    /// started from now on get it.
    pub fn create_brigade_skill(&self) -> Result<BrigadeSkill> {
        let path = self.brigade_override_path();
        if user_copy(&path).is_none() {
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, SKILL));
            if let Err(error) = written {
                return Err(Error::Invalid(format!("{}: {error}", path.display())));
            }
            self.brigade_plugin();
        }
        Ok(self.brigade_skill())
    }

    /// Removes the user's copy: sessions started from now on get the shipped skill.
    pub fn reset_brigade_skill(&self) -> Result<BrigadeSkill> {
        let path = self.brigade_override_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::Invalid(format!("{}: {error}", path.display()))),
        }
        self.brigade_plugin();
        Ok(self.brigade_skill())
    }
}

/// The user's copy, unless it is missing or empty.
fn user_copy(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .filter(|text| !text.trim().is_empty())
}

/// Writes `<dir>/.claude-plugin/plugin.json`, `<dir>/skills/brigade/SKILL.md` and
/// `<dir>/skills/fusion/SKILL.md`.
fn write_plugin(dir: &Path, skill: &str) -> std::io::Result<PathBuf> {
    let manifest = serde_json::json!({
        "name": "brigade",
        "description": "How a Brigadier session runs workers of its own, and fuses models' advice.",
        "version": env!("CARGO_PKG_VERSION"),
    });
    write_if_changed(
        &dir.join(".claude-plugin/plugin.json"),
        &serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )?;
    write_if_changed(&dir.join("skills/brigade/SKILL.md"), skill)?;
    write_if_changed(&dir.join("skills/fusion/SKILL.md"), FUSION)?;
    Ok(dir.to_path_buf())
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_is_short_and_says_what_it_is_for() {
        assert!(
            SKILL.lines().count() <= 150,
            "{} lines",
            SKILL.lines().count()
        );
        assert!(SKILL.starts_with("---\nname: brigade\ndescription: "));
        for command in [
            "brigadier models",
            "brigadier start",
            "brigadier wait",
            "brigadier send",
        ] {
            assert!(SKILL.contains(command), "{command}");
        }
        // A Chat runs workers too (S28).
        assert!(!SKILL.contains("needs a project"));
        assert!(SKILL.contains("--repo"));
        // S22b's four lines: decisions travel to workers, derived documents and the user.
        for line in [
            "The brief carries the rules that bear on its job, verbatim, with their ledger ids",
            "A plan or brief built from decisions cites their ids. Lines of your own say \"proposed\", and a gap the decisions don't cover goes to `brigadier ask`, never a silent fill.",
            "run `brigadier ledger check <file>` on it, and give its reviewer the ledger with the question \"what is missing, changed or added?\"",
            "When the user writes after a gap, raise the oldest open question first",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        assert!(SKILL.contains("the fusion skill"));
        // Reviews in proportion (the w123 bench: a two-file job tripled in time under them).
        for line in [
            "- Default: do it yourself.",
            "Adversarial review, for work worth one (several files, no tests of its own, risky, or the user asked)",
            "A small change with its tests passing lands without a review or a critique.",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
    }

    #[test]
    fn the_fusion_skill_consults_each_cli_through_its_own_row() {
        assert!(FUSION.starts_with("---\nname: fusion\ndescription: "));
        assert!(FUSION.lines().count() <= 150);
        for line in [
            "/fusion <question>",
            "`fusion-claude`, `fusion-codex`",
            "brigadier start \"<brief>\" --lean --work fusion-claude",
            "Answer only; change no file.",
            "One cross round.",
            "You judge; never count votes.",
        ] {
            assert!(FUSION.contains(line), "{line}");
        }
    }
}
