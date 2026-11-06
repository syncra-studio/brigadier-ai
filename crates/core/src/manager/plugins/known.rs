//! What the user has already been asked about: `<data_dir>/plugins-known.json`. New items stay
//! off until the user chooses, and Refresh is the only thing that asks. This holds no settings
//! (`plugins.json` does, and an accepted item saves none, so its derived scope stays derived).
//!
//! A row's id has a hash of its content, so an edit or an update changes it. Saved settings
//! follow a definition to its new id by kind, name and where it comes from (several CLIs can
//! have different definitions of one name, each with its own answer). When that's unclear the
//! item counts as new: off and asked, so nothing declined can come back on by itself.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;
use brigadier_registry::Catalog;
use brigadier_registry::catalog::{Content, Kind, Row, RowsRead};
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
    pub agents: Vec<ProviderKind>,
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
    /// Config files that couldn't be read: while there are any, nothing is reported gone.
    pub unread: Vec<String>,
}

/// Where a definition comes from: what still identifies it once its content, and so its row's
/// id, has changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Origin {
    agent: ProviderKind,
    plugin: Option<String>,
    project: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Item {
    kind: PluginKind,
    name: String,
    /// The user chose Keep when its files went: it's listed as missing, and not asked again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    kept: bool,
    /// Empty in a list saved before these were kept: such an entry matches only its own id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    origins: Vec<Origin>,
    /// Here only so its settings can follow it (a project's own item): Refresh doesn't ask.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    exempt: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Known {
    items: BTreeMap<String, Item>,
    /// Stands in while a user config can't be read: never saved, so the first list is complete.
    #[serde(skip)]
    provisional: bool,
}

/// What's already been decided about a row with no saved choice of its own.
enum Past {
    Chosen(Choice),
    Accepted,
    Unknown,
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

/// Where `row`'s definitions come from. Old sharing mirrors don't count if it has originals, as
/// in [`Choice::default_for`].
fn origins(row: &Row) -> Vec<Origin> {
    let mirrored = |origin: &brigadier_registry::catalog::Provenance| {
        origin
            .plugin
            .as_deref()
            .is_some_and(|id| id.ends_with("@brigadier"))
    };
    let original = row
        .definitions
        .iter()
        .any(|definition| !mirrored(&definition.provenance));
    let mut found: Vec<Origin> = Vec::new();
    for origin in row
        .definitions
        .iter()
        .map(|definition| &definition.provenance)
    {
        let Some(agent) = origin.agent else { continue };
        if original && mirrored(origin) {
            continue;
        }
        let origin = Origin {
            agent,
            plugin: origin.plugin.clone(),
            project: origin.project.clone(),
        };
        if !found.contains(&origin) {
            found.push(origin);
        }
    }
    found
}

/// Whether the row with id `id` still has a definition from `origin`.
fn still_holds(rows: &[Row], id: &str, origin: &Origin) -> bool {
    rows.iter()
        .find(|row| row.id == id)
        .is_some_and(|row| origins(row).contains(origin))
}

/// For an item Refresh never asks about, with no history of its own: a same-named saved "no"
/// whose row has left still holds, as there's no prompt to answer. Hidden wins over off.
fn fallback_choice(rows: &[Row], row: &Row, choices: &Choices) -> Option<(String, Choice)> {
    let earlier = || {
        choices.0.iter().filter(|(id, _)| {
            **id != row.id
                && same_item(id, kind_of(row), &row.name)
                && !rows.iter().any(|other| other.id == **id)
        })
    };
    earlier()
        .find(|(_, choice)| choice.removed == Some(true))
        .or_else(|| earlier().find(|(_, choice)| !choice.on))
        .map(|(id, choice)| (id.clone(), choice.clone()))
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
        if self.provisional {
            return Ok(());
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
        crate::manager::session_dir::write_atomic(&data_dir.join(FILE), &bytes)
            .map_err(|error| format!("Couldn't save {FILE}: {error}"))
    }

    /// The saved list, or the first one: everything there is on the first read counts as
    /// accepted, so nobody starts with a giant prompt. The caller holds `CATALOG_WRITES`.
    pub(super) fn read_or_start(
        data_dir: &Path,
        read: impl FnOnce() -> RowsRead,
    ) -> Result<Self, String> {
        if let Some(known) = Self::load(data_dir)? {
            return Ok(known);
        }
        let read = read();
        Self::start(data_dir, &read.rows, read.user_setup_read())
    }

    fn start(data_dir: &Path, rows: &[Row], complete: bool) -> Result<Self, String> {
        let mut known = Self {
            provisional: !complete,
            ..Self::default()
        };
        for row in rows.iter().filter(|row| counted(row)) {
            known.remember(rows, row);
        }
        known.save(data_dir)?;
        Ok(known)
    }

    /// `read_or_start`, for a caller that doesn't hold the lock and has read the rows.
    pub(super) fn read(data_dir: &Path, rows: &[Row], complete: bool) -> Result<Self, String> {
        if let Some(known) = Self::load(data_dir)? {
            return Ok(known);
        }
        let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(known) = Self::load(data_dir)? {
            return Ok(known);
        }
        Self::start(data_dir, rows, complete)
    }

    pub(super) fn save_if_changed(&self, data_dir: &Path, before: &Self) -> Result<(), String> {
        if self == before {
            return Ok(());
        }
        self.save(data_dir)
    }

    /// The user has chosen about `row`, or it was there from the start. It takes over the
    /// origins of the earlier versions of it; `rows` tells those from what's still there.
    pub(super) fn remember(&mut self, rows: &[Row], row: &Row) {
        let (kind, mine) = (kind_of(row), origins(row));
        let kept = self.is_kept(kind, &row.name);
        self.items.retain(|id, item| {
            if *id == row.id || item.kind != kind || item.name != row.name {
                return true;
            }
            let had = !item.origins.is_empty();
            item.origins
                .retain(|origin| !mine.contains(origin) || still_holds(rows, id, origin));
            !had || !item.origins.is_empty()
        });
        self.items.insert(
            row.id.clone(),
            Item {
                kind,
                name: row.name.clone(),
                kept,
                origins: mine,
                exempt: !counted(row),
            },
        );
    }

    fn is_kept(&self, kind: PluginKind, name: &str) -> bool {
        self.items
            .values()
            .any(|item| item.kind == kind && item.name == name && item.kept)
    }

    /// The ids that were `row`, from where it comes from now, and aren't any more.
    fn earlier_ids(&self, rows: &[Row], row: &Row) -> Vec<&String> {
        let (kind, mine) = (kind_of(row), origins(row));
        self.items
            .iter()
            .filter(|(id, item)| {
                **id != row.id
                    && item.kind == kind
                    && item.name == row.name
                    && item
                        .origins
                        .iter()
                        .any(|origin| mine.contains(origin) && !still_holds(rows, id, origin))
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// If its earlier versions don't all say the same, nothing is known about it.
    fn past(&self, rows: &[Row], row: &Row, choices: &Choices) -> Past {
        if self.items.contains_key(&row.id) {
            return Past::Accepted;
        }
        let answers: Vec<_> = self
            .earlier_ids(rows, row)
            .into_iter()
            .map(|id| choices.0.get(id))
            .collect();
        match answers.first() {
            Some(first) if answers.iter().all(|answer| answer == first) => {
                first.map_or(Past::Accepted, |choice| Past::Chosen(choice.clone()))
            }
            _ => Past::Unknown,
        }
    }

    /// Nobody has chosen about it: off, in launches and on the page, until someone does.
    fn is_new(&self, rows: &[Row], row: &Row, choices: &Choices) -> bool {
        counted(row)
            && !choices.0.contains_key(&row.id)
            && matches!(self.past(rows, row, choices), Past::Unknown)
    }

    /// `choices` as launches and the page read them, never saved: each new item is off, and an
    /// item with a new id has the settings of its earlier one.
    pub(super) fn apply(&self, rows: &[Row], choices: &mut Choices) {
        let saved = choices.clone();
        for row in rows {
            if saved.0.contains_key(&row.id) {
                continue;
            }
            let choice = match self.past(rows, row, &saved) {
                Past::Accepted => None,
                Past::Chosen(choice) => Some(choice),
                Past::Unknown if counted(row) => Some(Choice {
                    on: false,
                    ..Choice::default_for(row)
                }),
                Past::Unknown => fallback_choice(rows, row, &saved).map(|(_, choice)| choice),
            };
            if let Some(choice) = choice {
                choices.0.insert(row.id.clone(), choice);
            }
        }
    }

    /// Saved settings move to `row`'s id before anything saved is changed, and those of its
    /// earlier versions go. Another CLI's definition of the name keeps its own.
    pub(super) fn settle(&self, rows: &[Row], row: &Row, choices: &mut Choices) {
        let saved = choices.clone();
        if !saved.0.contains_key(&row.id) {
            match self.past(rows, row, &saved) {
                Past::Chosen(choice) => {
                    choices.0.insert(row.id.clone(), choice);
                }
                Past::Unknown if !counted(row) => {
                    if let Some((id, choice)) = fallback_choice(rows, row, &saved) {
                        choices.0.remove(&id);
                        choices.0.insert(row.id.clone(), choice);
                    }
                }
                _ => {}
            }
        }
        for id in self.earlier_ids(rows, row) {
            choices.0.remove(id);
        }
    }

    /// Everything in the list whose files have left every CLI, by kind and name; a hidden one
    /// stays hidden without being asked about. Nothing, while a config can't be read: what it
    /// holds is unknown, and a broken server entry is still there.
    fn left(&self, read: &RowsRead, choices: &Choices) -> Vec<(PluginKind, &str, bool)> {
        if !read.unread_files().is_empty() {
            return Vec::new();
        }
        let mut left: Vec<(PluginKind, &str, bool)> = Vec::new();
        for (id, item) in &self.items {
            let hidden = choices
                .0
                .get(id)
                .is_some_and(|choice| choice.removed == Some(true));
            if !item.exempt
                && !hidden
                && !in_a_cli(&read.rows, item.kind, &item.name)
                && !(item.kind == PluginKind::Server && read.has_broken_server(&item.name))
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

    fn gone(&self, read: &RowsRead, choices: &Choices, kept: bool) -> Vec<PluginGone> {
        self.left(read, choices)
            .into_iter()
            .filter(|(_, _, is_kept)| *is_kept == kept)
            .map(|(kind, name, _)| PluginGone {
                key: item_key(kind, name),
                kind,
                name: name.to_owned(),
            })
            .collect()
    }

    fn changes(&self, read: &RowsRead, choices: &Choices) -> PluginChanges {
        let rows = &read.rows;
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
            gone: self.gone(read, choices, false),
            unread: read
                .unread_files()
                .into_iter()
                .map(|file| file.display().to_string())
                .collect(),
        }
    }

    /// The page's lists beside its rows: what's missing and what's hidden.
    pub(super) fn fill_view(
        &self,
        read: &RowsRead,
        choices: &Choices,
        view: &mut super::CatalogView,
    ) {
        let missing = self.gone(read, choices, true);
        let hidden: Vec<_> = read
            .rows
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
        read: &RowsRead,
        choices: &mut Choices,
    ) {
        if !read.unread_files().is_empty() {
            return;
        }
        for (kind, name) in removed {
            if !in_a_cli(&read.rows, *kind, name) {
                self.forget(*kind, name, choices);
            }
        }
    }
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
) -> RowsRead {
    let paths: Vec<_> = projects.iter().map(|(_, _, path)| path.clone()).collect();
    Catalog::read_rows_checked(registry.roots(), &paths, data_dir)
}

impl crate::manager::SessionManager {
    fn manager(&self) -> crate::Result<std::sync::Arc<Self>> {
        self.me
            .upgrade()
            .ok_or_else(|| crate::Error::Invalid("The daemon is shutting down.".into()))
    }

    fn plugin_rows_and_known(&self) -> crate::Result<(RowsRead, Known, Choices)> {
        let read = self.plugin_catalog_read();
        let known =
            Known::read_or_start(&self.data_dir, || read.clone()).map_err(crate::Error::Invalid)?;
        let choices = Choices::load(&self.data_dir).map_err(crate::Error::Invalid)?;
        Ok((read, known, choices))
    }

    /// Refresh: what's new or gone since the user last chose. Asks nobody and changes no
    /// choice. Nothing else calls this: no page open, launch or watcher.
    pub async fn check_plugin_changes(&self) -> crate::Result<PluginChanges> {
        let manager = self.manager()?;
        crate::manager::blocking(move || {
            let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
            let (read, known, choices) = manager.plugin_rows_and_known()?;
            Ok(known.changes(&read, &choices))
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
            let (read, mut known, mut choices) = manager.plugin_rows_and_known()?;
            let rows = &read.rows;
            let find = |id: &String| rows.iter().find(|row| row.id == *id);
            for row in add.iter().filter_map(find) {
                known.settle(rows, row, &mut choices);
                known.remember(rows, row);
            }
            for row in decline.iter().filter_map(find) {
                known.settle(rows, row, &mut choices);
                choices.hide(row);
                known.remember(rows, row);
            }
            let left: Vec<_> = known
                .left(&read, &choices)
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
            let (read, mut known, mut choices) = manager.plugin_rows_and_known()?;
            let rows = &read.rows;
            let row = rows
                .iter()
                .find(|row| row.id == row_id)
                .ok_or_else(|| crate::Error::Invalid(format!("No plugin {row_id}")))?;
            known.settle(rows, row, &mut choices);
            if let Some(choice) = choices.0.get_mut(&row.id) {
                choice.removed = None;
            }
            known.remember(rows, row);
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
