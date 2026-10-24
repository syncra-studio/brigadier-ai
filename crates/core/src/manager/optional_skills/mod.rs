//! Optional catalog skills. Their files live only in Brigadier's data directory; selected
//! plugins reach Claude through `--plugin-dir` and Codex through its app-server's extra roots.

use std::path::{Path, PathBuf};

use super::SessionManager;
use super::brigade::write_if_changed;
use super::integrations::SkillView;
use crate::model::{ConversationId, OPTIONAL_SKILL_IDS, ProjectId, Settings};

const LICENSE: &str = include_str!("LICENSE");
const RETRO: &str = include_str!("retro/SKILL.md");
const WRITING: &str = include_str!("writing-for-agents/SKILL.md");
const MECHANICS: &str = include_str!("writing-for-agents/SKILL-MECHANICS.md");
const RETRO_POLICY: &str = include_str!("retro/agents/openai.yaml");
const WRITING_POLICY: &str = include_str!("writing-for-agents/agents/openai.yaml");

fn plugin_dir(data: &Path, name: &str) -> PathBuf {
    data.join("optional-skills").join(name)
}

fn write_plugin(data: &Path, name: &str, text: &str, policy: &str) -> std::io::Result<PathBuf> {
    let dir = plugin_dir(data, name);
    let skill = dir.join("skills").join(name);
    let manifest = serde_json::json!({
        "name": name,
        "version": env!("CARGO_PKG_VERSION"),
    });
    write_if_changed(
        &dir.join(".claude-plugin/plugin.json"),
        &manifest.to_string(),
    )?;
    write_if_changed(&skill.join("SKILL.md"), text)?;
    write_if_changed(&skill.join("agents/openai.yaml"), policy)?;
    // Every distributed copy carries its permission and copyright notice.
    write_if_changed(&skill.join("LICENSE"), LICENSE)?;
    if name == "writing-for-agents" {
        write_if_changed(&skill.join("SKILL-MECHANICS.md"), MECHANICS)?;
    }
    Ok(dir)
}

fn write_plugins(data: &Path, on: [bool; 2]) -> std::io::Result<Vec<PathBuf>> {
    [
        ("retro", RETRO, RETRO_POLICY),
        ("writing-for-agents", WRITING, WRITING_POLICY),
    ]
    .into_iter()
    .zip(on)
    .filter(|(_, enabled)| *enabled)
    .map(|((name, text, policy), _)| write_plugin(data, name, text, policy))
    .collect()
}

/// Settings uses the same skill rows and switches as the rest of the catalog. The description
/// supplies session-file navigation without changing either skill's text.
pub(super) fn catalog(data: &Path, settings: &Settings) -> Vec<SkillView> {
    // The catalog's Show in Finder paths exist even before the user enables a skill. Merely
    // keeping these files in Brigadier's data directory exposes them to neither CLI.
    if let Err(error) = write_plugins(data, [true, true]) {
        tracing::warn!(%error, "couldn't write the optional skills catalog");
    }
    [
        ("retro", "Conduct a retrospective on a coding session. Type /retro to use it; includes writing-for-agents. Brigadier uses the CLIs' own session files: Claude in $CLAUDE_CONFIG_DIR/projects (default ~/.claude/projects), Codex in $CODEX_HOME/sessions or archived_sessions (default ~/.codex)."),
        ("writing-for-agents", "Write documents agents consume. Included when retro is enabled; can also be enabled on its own."),
    ].into_iter().zip(OPTIONAL_SKILL_IDS).map(|((name, description), id)| {
        SkillView {
            id: (*id).to_owned(),
            name: name.to_owned(),
            description: description.to_owned(),
            path: plugin_dir(data, name).join("skills").join(name).display().to_string(),
            locations: Vec::new(),
            plugin: None,
            native_to: Vec::new(),
            off: settings.skills_off.iter().any(|off| off == id),
            claude_off_by_name: false,
        }
    }).collect()
}

fn selected(
    settings: &Settings,
    project: Option<&ProjectId>,
    keep: Option<&[String]>,
) -> [bool; 2] {
    let mut on: [bool; 2] = std::array::from_fn(|index| {
        let id = OPTIONAL_SKILL_IDS[index];
        let name = id.rsplit(':').next().unwrap_or(id);
        !settings.skill_off(id, project)
            && keep.is_none_or(|keep| keep.iter().any(|kept| kept == id || kept == name))
    });
    // Retro's first step invokes the writing guide. It is a dependency even when the user
    // hasn't enabled the writing guide independently (including a lean worker keeping retro).
    on[1] |= on[0];
    on
}

impl SessionManager {
    pub(super) fn optional_skill_plugins(
        &self,
        conversation: &ConversationId,
        keep: Option<&[String]>,
    ) -> Vec<PathBuf> {
        let project = self
            .core
            .conversation(conversation)
            .ok()
            .and_then(|c| c.project_id);
        let on = selected(&self.core.settings(), project.as_ref(), keep);
        match write_plugins(&self.data_dir, on) {
            Ok(plugins) => plugins,
            Err(error) => {
                tracing::warn!(%error, "couldn't write optional skills");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProjectSwitches;

    #[test]
    fn catalog_is_off_by_default_and_points_at_native_session_files() {
        let settings = Settings::default();
        let data = std::env::temp_dir().join(format!(
            "20261022-0805-take-over-5-sessions-w189-catalog-{}",
            uuid::Uuid::new_v4()
        ));
        let rows = catalog(&data, &settings);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.off && row.native_to.is_empty()));
        assert_eq!(rows[0].id, OPTIONAL_SKILL_IDS[0]);
        assert_eq!(rows[1].id, OPTIONAL_SKILL_IDS[1]);
        assert!(
            rows.iter()
                .all(|row| Path::new(&row.path).join("SKILL.md").is_file())
        );
        for path in [
            "$CLAUDE_CONFIG_DIR/projects",
            "~/.claude/projects",
            "$CODEX_HOME/sessions",
            "archived_sessions",
            "~/.codex",
        ] {
            assert!(rows[0].description.contains(path), "{path}");
        }
        assert!(RETRO.contains("disable-model-invocation: true\n"));
        assert!(RETRO_POLICY.contains("allow_implicit_invocation: false\n"));
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn selection_honors_projects_lean_workers_and_the_dependency() {
        let mut settings = Settings::default();
        assert_eq!(selected(&settings, None, None), [false, false]);
        settings.skills_off.retain(|id| id != OPTIONAL_SKILL_IDS[0]);
        assert_eq!(selected(&settings, None, None), [true, true]);
        assert_eq!(selected(&settings, None, Some(&[])), [false, false]);
        assert_eq!(
            selected(&settings, None, Some(&["retro".into()])),
            [true, true]
        );
        let project = ProjectId("one".into());
        settings.project_switches.push(ProjectSwitches {
            project_id: project.clone(),
            servers_on: vec![],
            servers_off: vec![],
            skills_on: vec![],
            skills_off: vec![OPTIONAL_SKILL_IDS[0].into()],
        });
        assert_eq!(selected(&settings, Some(&project), None), [false, false]);
        settings.project_switches[0]
            .skills_on
            .push(OPTIONAL_SKILL_IDS[1].into());
        assert_eq!(selected(&settings, Some(&project), None), [false, true]);
        settings = Settings::default();
        settings.project_switches.push(ProjectSwitches {
            project_id: project.clone(),
            servers_on: vec![],
            servers_off: vec![],
            skills_on: vec![OPTIONAL_SKILL_IDS[0].into()],
            skills_off: vec![],
        });
        assert_eq!(selected(&settings, Some(&project), None), [true, true]);
        assert_eq!(selected(&settings, None, None), [false, false]);
    }

    #[test]
    fn plugin_copies_keep_the_payload_policy_reference_and_license() {
        let data = std::env::temp_dir().join(format!(
            "20261022-0805-take-over-5-sessions-w189-optional-{}",
            uuid::Uuid::new_v4()
        ));
        for (name, text, policy) in [
            ("retro", RETRO, RETRO_POLICY),
            ("writing-for-agents", WRITING, WRITING_POLICY),
        ] {
            let plugin = write_plugin(&data, name, text, policy).unwrap();
            let skill = plugin.join("skills").join(name);
            assert_eq!(
                std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
                text
            );
            assert_eq!(
                std::fs::read_to_string(skill.join("LICENSE")).unwrap(),
                LICENSE
            );
            assert_eq!(
                std::fs::read_to_string(skill.join("agents/openai.yaml")).unwrap(),
                policy
            );
        }
        assert_eq!(
            std::fs::read_to_string(
                plugin_dir(&data, "writing-for-agents")
                    .join("skills/writing-for-agents/SKILL-MECHANICS.md")
            )
            .unwrap(),
            MECHANICS
        );
        assert!(!data.join(".claude").exists());
        assert!(!data.join(".codex").exists());
        assert!(!data.join(".agents").exists());
        std::fs::remove_dir_all(data).unwrap();
    }
}
