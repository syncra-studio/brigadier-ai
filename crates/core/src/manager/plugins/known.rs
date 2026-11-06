//! What the user has already been asked about: `<data_dir>/plugins-known.json`. New items stay
//! off until the user chooses, and Refresh is the only thing that asks. This holds no settings
//! (`plugins.json` does, and an accepted item saves none, so its derived scope stays derived).
//!
//! A row's id has a hash of its content, so an edit or an update changes it: an item here is
//! its kind and name, and saved settings follow it to its new id.
use std::collections::BTreeMap;
use std::path::Path;

use brigadier_registry::Catalog;
use brigadier_registry::catalog::{Content, Kind, Row};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::PluginKind;
use super::choices::{Choice, Choices, Scope};
use super::ops::CATALOG_WRITES;

const FILE: &str = "plugins-known.json";

/// An item Refresh found, off until the user adds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginFound {
    pub key: String,
    pub kind: PluginKind,
    pub name: String,
    /// Its command, address or folder; never a value.
    pub detail: String,
    pub agents: Vec<brigadier_providers::ProviderKind>,
}

/// An item whose files have left every CLI: only its name is left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginGone {
    pub key: String,
    pub kind: PluginKind,
    pub name: String,
}

/// An item the user kept out of Brigadier; its files are untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginHidden {
    pub key: String,
    pub kind: PluginKind,
    pub name: String,
    pub detail: String,
}

/// What a Refresh found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginChanges {
    pub found: Vec<PluginFound>,
    pub gone: Vec<PluginGone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Item {
    kind: PluginKind,
    name: String,
    /// The user chose Keep when its files went: it's listed as missing, and not asked again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    kept: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Known {
    items: BTreeMap<String, Item>,
}

fn kind_of(row: &Row) -> PluginKind {
    if row.kind == Kind::Server {
        PluginKind::Server
    } else {
        PluginKind::Skill
    }
}

/// `skill:name`: a row id without its hash, and a gone item's key.
fn item_key(kind: PluginKind, name: &str) -> String {
    let tag = if kind == PluginKind::Server {
        "server"
    } else {
        "skill"
    };
    format!("{tag}:{name}")
}

fn same_item(id: &str, kind: PluginKind, name: &str) -> bool {
    id.rsplit_once(':')
        .is_some_and(|(item, _)| item == item_key(kind, name))
}

/// Whether Refresh asks about it. A project's own item comes and goes with its branch and is
/// let in by trusting the project; an import offer isn't in any CLI yet; a provider's stays
/// under its CLI's control. All of them keep the default they had.
fn counted(row: &Row) -> bool {
    matches!(Choice::default_for(row).scope, Scope::Global(_))
        && row.definitions.iter().any(|definition| {
            let origin = &definition.provenance;
            origin.agent.is_some() && origin.project.is_none() && !origin.provider_supplied()
        })
}

/// Whether any CLI still has this item, in any project.
fn in_a_cli(rows: &[Row], kind: PluginKind, name: &str) -> bool {
    rows.iter().any(|row| {
        kind_of(row) == kind
            && row.name == name
            && row
                .definitions
                .iter()
                .any(|definition| definition.provenance.agent.is_some())
    })
}

/// The saved settings of an earlier version of `row`: an id that's no longer read.
fn earlier_choice(rows: &[Row], row: &Row, choices: &Choices) -> Option<Choice> {
    choices
        .0
        .iter()
        .find(|(id, _)| is_earlier(id, rows, row))
        .map(|(_, choice)| choice.clone())
}

fn is_earlier(id: &str, rows: &[Row], row: &Row) -> bool {
    id != row.id
        && same_item(id, kind_of(row), &row.name)
        && !rows.iter().any(|other| other.id == id)
}

/// Its command, address or folder, as the page shows it.
fn detail(row: &Row) -> String {
    let Some(definition) = row.definitions.first() else {
        return String::new();
    };
    match &definition.content {
        Content::Server(server) => server.written.url.clone().unwrap_or_else(|| {
            std::iter::once(server.written.command.clone().unwrap_or_default())
                .chain(server.written.args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ")
        }),
        Content::Skill { path, .. } => path.display().to_string(),
    }
}

impl Known {
    fn load(data_dir: &Path) -> Result<Option<Self>, String> {
        let file = data_dir.join(FILE);
        match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| format!("Couldn't read {}: {error}", file.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("Couldn't read {}: {error}", file.display())),
        }
    }

    fn save(&self, data_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
        crate::manager::session_dir::write_atomic(&data_dir.join(FILE), &bytes)
            .map_err(|error| format!("Couldn't save {FILE}: {error}"))
    }

    /// The saved list, or the first one: everything there is on the first read counts as
    /// accepted, so nobody starts with a giant prompt. The caller holds `CATALOG_WRITES`.
    pub(super) fn read_or_start(
        data_dir: &Path,
        rows: impl FnOnce() -> Vec<Row>,
    ) -> Result<Self, String> {
        if let Some(known) = Self::load(data_dir)? {
            return Ok(known);
        }
        let mut known = Self::default();
        for row in rows() {
            known.remember(&row);
        }
        known.save(data_dir)?;
        Ok(known)
    }

    /// `read_or_start`, for a caller that doesn't hold the lock.
    pub(super) fn read(data_dir: &Path, rows: &[Row]) -> Result<Self, String> {
        if let Some(known) = Self::load(data_dir)? {
            return Ok(known);
        }
        let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
        Self::read_or_start(data_dir, || rows.to_vec())
    }

    pub(super) fn save_if_changed(&self, data_dir: &Path, before: &Self) -> Result<(), String> {
        if self == before {
            return Ok(());
        }
        self.save(data_dir)
    }

    /// The user has chosen about `row`, or it was there from the start.
    pub(super) fn remember(&mut self, row: &Row) {
        if counted(row) {
            let kept = self.is_kept(kind_of(row), &row.name);
            self.items.insert(
                row.id.clone(),
                Item {
                    kind: kind_of(row),
                    name: row.name.clone(),
                    kept,
                },
            );
        }
    }

    fn has(&self, row: &Row) -> bool {
        let kind = kind_of(row);
        self.items
            .values()
            .any(|item| item.kind == kind && item.name == row.name)
    }

    fn is_kept(&self, kind: PluginKind, name: &str) -> bool {
        self.items
            .values()
            .any(|item| item.kind == kind && item.name == name && item.kept)
    }

    /// Nobody has chosen about it: off, in launches and on the page, until someone does.
    fn is_new(&self, rows: &[Row], row: &Row, choices: &Choices) -> bool {
        counted(row)
            && !choices.0.contains_key(&row.id)
            && earlier_choice(rows, row, choices).is_none()
            && !self.has(row)
    }

    /// `choices` as launches and the page read them, never saved: each new item is off, and an
    /// item with a new id has the settings of its earlier one.
    pub(super) fn apply(&self, rows: &[Row], choices: &mut Choices) {
        let saved = choices.clone();
        for row in rows {
            if saved.0.contains_key(&row.id) {
                continue;
            }
            // Existing settings follow an item even when it now exists only in a project.
            // Being exempt from Refresh must not undo a saved off/hidden choice.
            if let Some(earlier) = earlier_choice(rows, row, &saved) {
                choices.0.insert(row.id.clone(), earlier);
            } else if counted(row) && !self.has(row) {
                choices.0.insert(
                    row.id.clone(),
                    Choice {
                        on: false,
                        ..Choice::default_for(row)
                    },
                );
            }
        }
    }

    /// Everything in the list whose files have left every CLI, by kind and name; a hidden one
    /// stays hidden without being asked about.
    fn left(&self, rows: &[Row], choices: &Choices) -> Vec<(PluginKind, &str, bool)> {
        let mut left: Vec<(PluginKind, &str, bool)> = Vec::new();
        for item in self.items.values() {
            let hidden = choices.0.iter().any(|(id, choice)| {
                choice.removed == Some(true) && same_item(id, item.kind, &item.name)
            });
            if !hidden
                && !in_a_cli(rows, item.kind, &item.name)
                && !left
                    .iter()
                    .any(|(kind, name, _)| *kind == item.kind && *name == item.name)
            {
                left.push((item.kind, &item.name, self.is_kept(item.kind, &item.name)));
            }
        }
        left.sort_by_key(|(_, name, _)| *name);
        left
    }

    fn gone(&self, rows: &[Row], choices: &Choices, kept: bool) -> Vec<PluginGone> {
        self.left(rows, choices)
            .into_iter()
            .filter(|(_, _, is_kept)| *is_kept == kept)
            .map(|(kind, name, _)| PluginGone {
                key: item_key(kind, name),
                kind,
                name: name.to_owned(),
            })
            .collect()
    }

    fn changes(&self, rows: &[Row], choices: &Choices) -> PluginChanges {
        PluginChanges {
            found: rows
                .iter()
                .filter(|row| self.is_new(rows, row, choices))
                .map(|row| {
                    let mut agents = Vec::new();
                    for agent in row
                        .definitions
                        .iter()
                        .filter_map(|definition| definition.provenance.agent)
                    {
                        if !agents.contains(&agent) {
                            agents.push(agent);
                        }
                    }
                    PluginFound {
                        key: row.id.clone(),
                        kind: kind_of(row),
                        name: row.name.clone(),
                        detail: detail(row),
                        agents,
                    }
                })
                .collect(),
            gone: self.gone(rows, choices, false),
        }
    }

    /// The page's lists beside its rows: what's missing and what's hidden.
    pub(super) fn fill_view(&self, rows: &[Row], choices: &Choices, view: &mut super::CatalogView) {
        let missing = self.gone(rows, choices, true);
        let hidden: Vec<_> = rows
            .iter()
            .filter(|row| {
                choices.is_removed(&row.id)
                    && row
                        .definitions
                        .iter()
                        .any(|definition| definition.provenance.agent.is_some())
            })
            .map(|row| PluginHidden {
                key: row.id.clone(),
                kind: kind_of(row),
                name: row.name.clone(),
                detail: detail(row),
            })
            .collect();
        if missing.is_empty() && hidden.is_empty() {
            return;
        }
        let bytes = serde_json::to_vec(&(&missing, &hidden)).expect("lists are serializable");
        let hash = brigadier_registry::catalog::fingerprint(&bytes);
        view.version = format!("{}-{hash}", view.version);
        view.missing = (!missing.is_empty()).then_some(missing);
        view.hidden = (!hidden.is_empty()).then_some(hidden);
    }

    /// Forgets `kind` and `name` now no CLI has them: this list, and their saved settings.
    fn forget(&mut self, kind: PluginKind, name: &str, choices: &mut Choices) {
        self.items
            .retain(|_, item| item.kind != kind || item.name != name);
        choices.0.retain(|id, _| !same_item(id, kind, name));
    }

    /// Library items that were just removed: they aren't gone from the computer behind the
    /// user's back, so Refresh doesn't ask about them.
    pub(super) fn forget_removed(
        &mut self,
        removed: &[(PluginKind, String)],
        rows: &[Row],
        choices: &mut Choices,
    ) {
        for (kind, name) in removed {
            if !in_a_cli(rows, *kind, name) {
                self.forget(*kind, name, choices);
            }
        }
    }
}

/// Saved settings move to `row`'s id before anything saved is changed, and earlier ids go.
pub(super) fn settle(rows: &[Row], row: &Row, choices: &mut Choices) {
    if !choices.0.contains_key(&row.id)
        && let Some(earlier) = earlier_choice(rows, row, choices)
    {
        choices.0.insert(row.id.clone(), earlier);
    }
    choices.0.retain(|id, _| !is_earlier(id, rows, row));
}

/// Everything the library's item `name` is read from, before it goes.
pub(super) fn library_items(rows: &[Row], root: &Path, name: &str) -> Vec<(PluginKind, String)> {
    let sources = [
        root.join("skills").join(name),
        root.join("servers").join(format!("{name}.json")),
        root.join("plugins").join(name),
    ];
    rows.iter()
        .filter(|row| {
            row.definitions.iter().any(|definition| {
                sources
                    .iter()
                    .any(|source| definition.provenance.file.starts_with(source))
            })
        })
        .map(|row| (kind_of(row), row.name.clone()))
        .collect()
}

/// The rows as of now, for a closure that doesn't have the manager.
pub(super) fn read_rows(
    registry: &brigadier_registry::Registry,
    projects: &[(String, String, std::path::PathBuf)],
    data_dir: &Path,
) -> Vec<Row> {
    let paths: Vec<_> = projects.iter().map(|(_, _, path)| path.clone()).collect();
    Catalog::read_rows_with_library(registry.roots(), &paths, data_dir)
}

impl crate::manager::SessionManager {
    fn manager(&self) -> crate::Result<std::sync::Arc<Self>> {
        self.me
            .upgrade()
            .ok_or_else(|| crate::Error::Invalid("The daemon is shutting down.".into()))
    }

    fn plugin_rows_and_known(&self) -> crate::Result<(Vec<Row>, Known, Choices)> {
        let rows = self.plugin_catalog_rows();
        let known =
            Known::read_or_start(&self.data_dir, || rows.clone()).map_err(crate::Error::Invalid)?;
        let choices = Choices::load(&self.data_dir).map_err(crate::Error::Invalid)?;
        Ok((rows, known, choices))
    }

    /// Refresh: what's new or gone since the user last chose. Asks nobody and changes no
    /// choice. Nothing else calls this: no page open, launch or watcher.
    pub async fn check_plugin_changes(&self) -> crate::Result<PluginChanges> {
        let manager = self.manager()?;
        crate::manager::blocking(move || {
            let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
            let (rows, known, choices) = manager.plugin_rows_and_known()?;
            Ok(known.changes(&rows, &choices))
        })
        .await
    }

    /// What the user chose for what Refresh found. A key that's since changed is skipped: the
    /// next Refresh asks about it again.
    pub async fn apply_plugin_changes(
        &self,
        add: Vec<String>,
        decline: Vec<String>,
        remove: Vec<String>,
        keep: Vec<String>,
    ) -> crate::Result<()> {
        let manager = self.manager()?;
        crate::manager::blocking(move || {
            let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
            let (rows, mut known, mut choices) = manager.plugin_rows_and_known()?;
            let find = |id: &String| rows.iter().find(|row| row.id == *id);
            for row in add.iter().filter_map(find) {
                settle(&rows, row, &mut choices);
                known.remember(row);
            }
            for row in decline.iter().filter_map(find) {
                settle(&rows, row, &mut choices);
                choices.hide(row);
                known.remember(row);
            }
            let left: Vec<_> = known
                .left(&rows, &choices)
                .into_iter()
                .map(|(kind, name, _)| (kind, name.to_owned()))
                .collect();
            for (kind, name) in left {
                let key = item_key(kind, &name);
                if remove.contains(&key) {
                    known.forget(kind, &name, &mut choices);
                } else if keep.contains(&key) {
                    for item in known
                        .items
                        .values_mut()
                        .filter(|item| item.kind == kind && item.name == name)
                    {
                        item.kept = true;
                    }
                }
            }
            choices
                .save(&manager.data_dir)
                .and_then(|()| known.save(&manager.data_dir))
                .map_err(crate::Error::Invalid)
        })
        .await
    }

    /// Add's Bring back: a hidden item returns as it was. Its native files were never touched.
    pub async fn restore_plugin(&self, row_id: String) -> crate::Result<()> {
        let manager = self.manager()?;
        crate::manager::blocking(move || {
            let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
            let (rows, mut known, mut choices) = manager.plugin_rows_and_known()?;
            let row = rows
                .iter()
                .find(|row| row.id == row_id)
                .ok_or_else(|| crate::Error::Invalid(format!("No plugin {row_id}")))?;
            settle(&rows, row, &mut choices);
            if let Some(choice) = choices.0.get_mut(&row.id) {
                choice.removed = None;
            }
            known.remember(row);
            choices
                .save(&manager.data_dir)
                .and_then(|()| known.save(&manager.data_dir))
                .map_err(crate::Error::Invalid)
        })
        .await
    }
}

#[cfg(test)]
#[path = "known_tests.rs"]
mod tests;
