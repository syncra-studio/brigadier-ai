//! Durable user choices. This module writes only `<data_dir>/plugins.json`.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use brigadier_registry::catalog::{Content, Kind, Row};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
pub enum Scope {
    Global(Global),
    Projects(Vec<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Global {
    Global,
}

impl Scope {
    pub fn contains(&self, project: Option<&Path>) -> bool {
        match self {
            Self::Global(_) => true,
            Self::Projects(paths) => {
                project.is_some_and(|project| paths.iter().any(|path| path == project))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Choice {
    pub on: bool,
    pub scope: Scope,
}

impl Choice {
    pub fn default_for(row: &Row) -> Self {
        let mut projects = Vec::new();
        // Retired sharing plugins are mirrors, not additional global sources. Their
        // installed caches can outlive cleanup; only originals determine default scope.
        let mirrored = |definition: &&brigadier_registry::catalog::Definition| {
            definition
                .provenance
                .plugin
                .as_deref()
                .is_some_and(|id| id.ends_with("@brigadier"))
        };
        let has_original = row
            .definitions
            .iter()
            .any(|definition| !mirrored(&definition));
        for definition in row
            .definitions
            .iter()
            .filter(|definition| !has_original || !mirrored(definition))
        {
            let Some(project) = &definition.provenance.project else {
                return Self {
                    on: true,
                    scope: Scope::Global(Global::Global),
                };
            };
            if !projects.contains(project) {
                projects.push(project.clone());
            }
        }
        projects.sort();
        Self {
            on: true,
            scope: Scope::Projects(projects),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Choices(pub BTreeMap<String, Choice>);

impl Choices {
    pub fn choice(&self, row: &Row) -> Choice {
        self.0
            .get(&row.id)
            .cloned()
            .unwrap_or_else(|| Choice::default_for(row))
    }

    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let file = data_dir.join("plugins.json");
        match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("Couldn't read {}: {error}", file.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("Couldn't read {}: {error}", file.display())),
        }
    }

    /// Fsync the replacement before renaming it, then fsync its directory. A failed read is
    /// never treated as an empty file. Callers serialize read/modify/save operations.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
        let temp = data_dir.join(format!(".plugins-{}.tmp", uuid::Uuid::new_v4()));
        let write = || -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, data_dir.join("plugins.json"))?;
            #[cfg(unix)]
            std::fs::File::open(data_dir)?.sync_all()?;
            Ok(())
        };
        write().map_err(|error| format!("Couldn't save plugins.json: {error}"))
    }
}

/// Translate the legacy record without mutating it. Explicit new choices always win when
/// merged by `import_sharing_file`. Multiple new rows matching an old name inherit its choice.
pub fn import_sharing(old: &serde_json::Value, rows: &[Row]) -> Result<Choices, String> {
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Legacy {
        chosen: BTreeMap<String, LegacyChoice>,
        off_scopes: BTreeMap<String, Option<Vec<PathBuf>>>,
        declined: Vec<Declined>,
        removed: Vec<serde_json::Value>,
        pending_removal: Vec<String>,
    }
    #[derive(Deserialize)]
    struct LegacyChoice {
        on: bool,
        scope: Option<Vec<PathBuf>>,
    }
    #[derive(Deserialize)]
    struct Declined {
        kind: String,
        name: String,
        endpoint: Option<String>,
        source: Option<String>,
    }
    fn scope(paths: &Option<Vec<PathBuf>>) -> Scope {
        paths
            .clone()
            .map_or(Scope::Global(Global::Global), Scope::Projects)
    }
    let old: Legacy = serde_json::from_value(old.clone())
        .map_err(|error| format!("Couldn't import .sharing.json: {error}"))?;
    let mut choices = Choices::default();
    for row in rows {
        let kind = if row.kind == Kind::Server {
            "server"
        } else {
            "skill"
        };
        let mut keys = vec![
            row.id.clone(),
            format!("{kind}:{}", row.name),
            format!("lib:{}", row.name),
        ];
        for definition in &row.definitions {
            if let Some(plugin) = &definition.provenance.plugin {
                let name = plugin
                    .rsplit_once('@')
                    .map_or(plugin.as_str(), |(name, _)| name);
                keys.push(format!("lib:{name}"));
                keys.push(format!("plugin:{name}"));
                keys.push(format!("plugin:{name}/{}", row.name));
            }
        }
        let explicit = keys.iter().find_map(|key| old.chosen.get(key));
        let remembered_off = keys.iter().find_map(|key| old.off_scopes.get(key));
        let removed = old.declined.iter().any(|item| {
            item.kind == kind
                && (item.name == row.name
                    || keys.contains(&format!("plugin:{}", item.name))
                    || keys.contains(&format!("lib:{}", item.name))
                    || item
                        .endpoint
                        .as_ref()
                        .is_some_and(|endpoint| row.endpoint.as_ref() == Some(endpoint) || row.definitions.iter().any(|definition| matches!(&definition.content, Content::Server(server) if super::model::endpoint(server) == *endpoint)))
                    || item.source.as_ref().is_some_and(|source| {
                        row.definitions
                            .iter()
                            .any(|definition| match &definition.content {
                                Content::Skill { path, .. } => {
                                    path.to_string_lossy() == source.as_str()
                                }
                                _ => false,
                            })
                    }))
        }) || old.removed.iter().any(|item| {
            item.as_str()
                .is_some_and(|key| keys.iter().any(|known| known == key))
        }) || old
            .pending_removal
            .iter()
            .any(|name| keys.contains(&format!("lib:{name}")));
        let mut choice = explicit
            .map(|choice| Choice {
                on: choice.on,
                scope: scope(&choice.scope),
            })
            .or_else(|| {
                remembered_off.map(|paths| Choice {
                    on: false,
                    scope: scope(paths),
                })
            })
            .unwrap_or_else(|| Choice::default_for(row));
        if removed {
            choice.on = false;
        }
        if explicit.is_some() || remembered_off.is_some() || removed {
            choices.0.insert(row.id.clone(), choice);
        }
    }
    Ok(choices)
}

/// Migration entry point: durable before cleanup can erase the old record. Safe to retry,
/// including after a partial cleanup; existing choices are never overwritten.
pub fn import_sharing_file(
    data_dir: &Path,
    sharing_file: &Path,
    rows: &[Row],
) -> Result<Choices, String> {
    let mut choices = Choices::load(data_dir)?;
    let old = match std::fs::read(sharing_file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("Couldn't read {}: {error}", sharing_file.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(choices),
        Err(error) => return Err(format!("Couldn't read {}: {error}", sharing_file.display())),
    };
    for (id, choice) in import_sharing(&old, rows)?.0 {
        choices.0.entry(id).or_insert(choice);
    }
    choices.save(data_dir)?;
    Ok(choices)
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn project_mcp_default_and_toggles_keep_scope_in_the_page_and_on_disk() {
        let home = std::env::temp_dir().join(format!("project-choice-{}", uuid::Uuid::new_v4()));
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let project = std::fs::canonicalize(project).unwrap();
        std::fs::write(
            project.join(".mcp.json"),
            r#"{"mcpServers":{"proj-everything":{"command":"npx","args":["-y","everything"]}}}"#,
        )
        .unwrap();
        let roots =
            brigadier_registry::Roots::from_vars([("HOME".into(), home.display().to_string())])
                .unwrap();
        let mut rows =
            brigadier_registry::Catalog::read_rows(&roots, std::slice::from_ref(&project));
        // The live fixture still has an old global Codex sharing copy of this server.
        let mut mirror = rows[0].definitions[0].clone();
        mirror.provenance.agent = Some(brigadier_registry::Agent::Codex);
        mirror.provenance.project = None;
        mirror.provenance.plugin = Some("proj-everything@brigadier".into());
        rows[0].definitions.insert(0, mirror);
        let row = rows
            .iter()
            .find(|row| row.name == "proj-everything")
            .unwrap();
        let expected = Scope::Projects(vec![project]);
        let mut choices = Choices::default();
        assert_eq!(
            super::super::CatalogView::new(&rows, &choices).rows[0]
                .choice
                .scope,
            expected
        );
        for on in [false, true] {
            let mut choice = choices.choice(row);
            choice.on = on;
            choices.0.insert(row.id.clone(), choice);
            choices.save(&home.join("data")).unwrap();
            choices = Choices::load(&home.join("data")).unwrap();
            let view = super::super::CatalogView::new(&rows, &choices);
            assert_eq!(view.rows[0].choice.scope, expected);
            assert_eq!(view.rows[0].choice.on, on);
        }
        // Real global originals still make a row global, and a legacy-only library row
        // still has its previous default. Neither case is confused with a project mirror.
        let mut global = row.clone();
        global.definitions[0].provenance.plugin = None;
        assert_eq!(
            Choice::default_for(&global).scope,
            Scope::Global(Global::Global)
        );
        let mut legacy_only = row.clone();
        legacy_only.definitions.truncate(1);
        assert_eq!(
            Choice::default_for(&legacy_only).scope,
            Scope::Global(Global::Global)
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
