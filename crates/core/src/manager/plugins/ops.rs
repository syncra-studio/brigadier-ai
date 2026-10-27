//! Plain data-dir Library operations and durable catalog choices.
use super::{PluginKind, provider};
use brigadier_providers::ProviderKind;
use brigadier_registry::adapter::adapters;
use brigadier_registry::add_parse;
use brigadier_registry::item::{Item, ServerSpec, SkillSource, SkillSpec, TransportSpec};
use brigadier_registry::library::{self, Origin};
use brigadier_registry::{Agent, Catalog, Secret};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use ts_rs::TS;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginAddItem {
    pub kind: PluginKind,
    /// Empty for a skill named by its `SKILL.md`, read once fetched.
    pub name: String,
    /// Its command, address, folder or repository.
    pub detail: String,
    pub remote: bool,
    /// Where it comes from, in plain words: "An MCP server from mcp.sentry.dev."
    pub summary: String,
    /// The names of its configured values; values never enter the page model.
    pub secrets: Vec<String>,
}

/// What [`check_add`] read in a paste.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginAddCheck {
    pub items: Vec<PluginAddItem>,
    /// Why it can't be added as it is, in plain words.
    pub error: Option<String>,
    /// It needs a name (a single server object, an address).
    pub needs_name: bool,
}

fn describe(item: &Item) -> PluginAddItem {
    match item {
        Item::Server(spec) => {
            let (detail, remote, secrets, summary) = match &spec.transport {
                TransportSpec::Stdio {
                    command, args, env, ..
                } => (
                    std::iter::once(command.as_str())
                        .chain(args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" "),
                    false,
                    env,
                    "An MCP server your agents start on this Mac.".to_owned(),
                ),
                TransportSpec::Http { url, headers } => {
                    let host = url
                        .split_once("://")
                        .map_or(url.as_str(), |(_, rest)| rest)
                        .split(['/', '?', '#'])
                        .next()
                        .unwrap_or_default()
                        .rsplit('@')
                        .next()
                        .unwrap_or_default();
                    let summary = if headers.is_empty() {
                        format!(
                            "An MCP server from {host}. If it asks, it signs in with your browser."
                        )
                    } else {
                        format!("An MCP server from {host}.")
                    };
                    (url.clone(), true, headers, summary)
                }
            };
            PluginAddItem {
                kind: PluginKind::Server,
                name: spec.name.clone(),
                detail,
                remote,
                summary,
                secrets: secrets.iter().map(|(name, _)| name.clone()).collect(),
            }
        }
        Item::Skill(spec) => {
            let (detail, summary) = match &spec.source {
                SkillSource::Folder(dir) => (
                    dir.display().to_string(),
                    "A skill from a folder on this Mac.".to_owned(),
                ),
                SkillSource::Git { url, .. } => (url.clone(), format!("A skill from {url}.")),
                SkillSource::SkillsSh(id) => (id.clone(), format!("A skill from skills.sh: {id}.")),
            };
            PluginAddItem {
                kind: PluginKind::Skill,
                name: spec.name.clone().unwrap_or_default(),
                detail,
                remote: false,
                summary,
                secrets: Vec::new(),
            }
        }
    }
}

/// What a paste would add, read before anything is added.
pub(crate) fn check_add(home: &Path, text: &str, name: Option<&str>) -> PluginAddCheck {
    match add_parse::parse(text, name, home) {
        Ok(items) => PluginAddCheck {
            items: items.iter().map(describe).collect(),
            error: None,
            needs_name: false,
        },
        Err(err) => PluginAddCheck {
            items: Vec::new(),
            needs_name: matches!(err, add_parse::AddError::NoName),
            error: Some(err.to_string()),
        },
    }
}

/// Every MCP server name the agents know, in every project, but those of `except`'s plugin.
/// A scratch folder for one fetch, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-plugin-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).map_err(|err| format!("Couldn't make a folder: {err}."))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `item`'s files, fetched: its skill's folder, and where it came from when fetched.
fn fetch(item: &Item, scratch: &Scratch) -> Result<(Option<PathBuf>, Option<Origin>), String> {
    let Item::Skill(spec) = item else {
        return Ok((None, None));
    };
    let dir = library::fetch_skill(&spec.source, &scratch.0)?;
    let origin = library::fetched_commit(&scratch.0).map(|commit| Origin {
        source: spec.source.clone(),
        commit,
    });
    Ok((Some(dir), origin))
}

/// The name a fetched skill goes by: the one asked for, its `SKILL.md`'s, else its folder's.
fn skill_name(spec: &SkillSpec, dir: &Path) -> String {
    spec.name
        .clone()
        .or_else(|| library::skill_name(dir))
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct AddOptions {
    /// Set or added to the one pasted server's private library entry.
    pub values: BTreeMap<String, String>,
}

/// Gives the one server in `items` `values`: each replaces the pasted value of its name, or
/// is added.
fn give_values(items: &mut [Item], values: &BTreeMap<String, String>) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    let [Item::Server(spec)] = items else {
        return Err(
            "Headers and environment values go with one MCP server: paste just that server."
                .to_owned(),
        );
    };
    let kept = match &mut spec.transport {
        TransportSpec::Stdio { env, .. } => env,
        TransportSpec::Http { headers, .. } => headers,
    };
    for (name, value) in values {
        let name = name.trim();
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '=') {
            return Err(format!(
                "\"{name}\" can't be a name: use letters, digits, - or _."
            ));
        }
        match kept.iter_mut().find(|(have, _)| have == name) {
            Some((_, secret)) => *secret = Secret::new(value.clone()),
            None => kept.push((name.to_owned(), Secret::new(value.clone()))),
        }
    }
    Ok(())
}

fn valid_library_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Use letters, digits, - or _ for a library name.".into());
    }
    Ok(())
}

fn write_private(path: &Path, value: &impl Serialize) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or("Missing library folder")?)
        .map_err(|e| e.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn server_value(spec: &ServerSpec) -> serde_json::Value {
    use serde_json::json;
    let values = |pairs: &[(String, Secret)]| {
        pairs
            .iter()
            .map(|(name, _)| (name.clone(), json!("")))
            .collect::<serde_json::Map<_, _>>()
    };
    let mut value = match &spec.transport {
        TransportSpec::Stdio {
            command,
            args,
            env,
            cwd,
        } => json!({"command":command,"args":args,"env":values(env),"cwd":cwd}),
        TransportSpec::Http { url, headers } => {
            json!({"type":"http","url":url,"headers":values(headers)})
        }
    };
    let names: Vec<_> = match &spec.transport {
        TransportSpec::Stdio { env, .. } => env,
        TransportSpec::Http { headers, .. } => headers,
    }
    .iter()
    .map(|(name, _)| name.clone())
    .collect();
    if !names.is_empty() {
        let id = format!(
            "{}-{}/{}",
            spec.name,
            uuid::Uuid::new_v4().simple(),
            spec.name
        );
        value["brigadierSecrets"] = json!({"id":id,"names":names});
        if matches!(spec.transport, TransportSpec::Http { .. }) {
            value["headersHelper"] = json!(format!("brigadier secret headers {id}"));
        }
    }
    if let Some(oauth) = &spec.oauth {
        value["oauth"] = json!({"clientId":oauth.client_id});
    }
    json!({"mcpServers": { &spec.name: value }})
}

fn copy_skill(from: &Path, to: &Path, ancestors: &mut Vec<PathBuf>) -> Result<(), String> {
    let canonical = std::fs::canonicalize(from).map_err(|e| e.to_string())?;
    if ancestors.contains(&canonical) {
        return Err("The skill contains a cyclic link.".into());
    }
    if from.is_dir() {
        ancestors.push(canonical);
        std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
        for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name() == ".git" {
                continue;
            }
            copy_skill(&entry.path(), &to.join(entry.file_name()), ancestors)?;
        }
        ancestors.pop();
    } else {
        std::fs::copy(from, to).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn library_name_exists(root: &Path, name: &str) -> bool {
    root.join("skills").join(name).exists()
        || root.join("servers").join(format!("{name}.json")).exists()
        || root.join("plugins").join(name).exists()
}

fn add_plain(
    root: &Path,
    items: &[Item],
    scope: Option<&[PathBuf]>,
) -> Result<Vec<String>, String> {
    if scope.is_some_and(<[PathBuf]>::is_empty) {
        return Err("Pick a project, or All projects.".into());
    }
    let mut keys = Vec::new();
    for item in items {
        let scratch = Scratch::new()?;
        let (skill_dir, origin) = fetch(item, &scratch)?;
        let name = match (item, skill_dir.as_deref()) {
            (Item::Server(spec), _) => spec.name.clone(),
            (Item::Skill(spec), Some(dir)) => skill_name(spec, dir),
            _ => return Err("The skill's files weren't fetched.".into()),
        };
        valid_library_name(&name)?;
        let target = match item {
            Item::Server(_) => root.join("servers").join(format!("{name}.json")),
            Item::Skill(_) => root.join("skills").join(&name),
        };
        if library_name_exists(root, &name) {
            return Err(format!("Your Library already has something called {name}."));
        }
        for path in [root.to_owned(), target.parent().unwrap().to_owned()] {
            if path
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err("The library folder cannot be a symbolic link.".into());
            }
        }
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        match item {
            Item::Server(spec) => write_private(&target, &server_value(spec))?,
            Item::Skill(_) => {
                let dir = skill_dir.as_deref().ok_or("Missing skill files")?;
                if !dir.join("SKILL.md").is_file() {
                    return Err("The folder has no SKILL.md.".into());
                }
                let staging = root.join(format!(".add-{}", uuid::Uuid::new_v4()));
                copy_skill(dir, &staging, &mut Vec::new())?;
                std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::rename(staging, &target).map_err(|e| e.to_string())?;
            }
        }
        if let Some(origin) = origin {
            write_private(&root.join("origins").join(format!("{name}.json")), &origin)?;
        }
        keys.push(format!("lib:{name}"));
    }
    Ok(keys)
}

fn remove_plain(root: &Path, name: &str) -> Result<(), String> {
    valid_library_name(name)?;
    if root
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err("The library folder cannot be a symbolic link.".into());
    }
    let skill = root.join("skills").join(name);
    let server = root.join("servers").join(format!("{name}.json"));
    let legacy = root.join("plugins").join(name);
    if !skill.exists() && !server.exists() && !legacy.exists() {
        return Err(format!(
            "{name} isn't in your Library; detected items can only be turned off."
        ));
    }
    for path in [&skill, &server, &legacy] {
        if !path.exists() {
            continue;
        }
        let actual = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        let library = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        if !actual.starts_with(&library)
            || path
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err("The library item points outside the library.".into());
        }
        if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
        .map_err(|e| e.to_string())?;
    }
    let origin = root.join("origins").join(format!("{name}.json"));
    if origin.is_file() {
        let actual = std::fs::canonicalize(&origin).map_err(|e| e.to_string())?;
        if !actual.starts_with(std::fs::canonicalize(root).map_err(|e| e.to_string())?) {
            return Err("The library origin points outside the library.".into());
        }
        std::fs::remove_file(origin).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn agent_of(kind: ProviderKind) -> Option<Agent> {
    adapters()
        .into_iter()
        .map(|adapter| adapter.agent())
        .find(|agent| provider(*agent) == kind)
}

// The page/CLI path serializes durable choices independently of legacy sync jobs.
static CATALOG_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl crate::manager::SessionManager {
    /// The page and launches must read this same provenance-preserving source.
    pub fn plugin_catalog_rows(&self) -> Vec<brigadier_registry::catalog::Row> {
        let projects: Vec<_> = self
            .plugin_projects()
            .into_iter()
            .map(|(_, _, path)| path)
            .collect();
        Catalog::read_rows_with_library(self.registry().roots(), &projects, &self.data_dir)
    }

    pub fn plugin_catalog(&self) -> crate::Result<super::CatalogView> {
        let rows = self.plugin_catalog_rows();
        let choices =
            super::choices::Choices::load(&self.data_dir).map_err(crate::Error::Invalid)?;
        let mut view = super::CatalogView::new(&rows, &choices);
        // Commands are display data only; env/header values never enter the page model.
        for (row, source) in view.rows.iter_mut().zip(&rows) {
            for (definition, source) in row.definitions.iter_mut().zip(&source.definitions) {
                if let brigadier_registry::catalog::Content::Server(server) = &source.content
                    && let Some(command) = &server.written.command
                {
                    definition.endpoint = Some(
                        std::iter::once(command.as_str())
                            .chain(server.written.args.iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
            }
        }
        Ok(view)
    }

    pub fn set_plugin_choice(&self, row_id: &str, on: bool) -> crate::Result<()> {
        self.edit_plugin_choice(row_id, Some(on), None)
    }

    pub fn set_plugin_choice_scope(
        &self,
        row_id: &str,
        scope: super::choices::Scope,
    ) -> crate::Result<()> {
        if let super::choices::Scope::Projects(paths) = &scope {
            let projects = self.plugin_projects();
            if paths.is_empty()
                || paths
                    .iter()
                    .any(|path| !projects.iter().any(|(_, _, known)| known == path))
            {
                return Err(crate::Error::Invalid(
                    "Pick registered, trusted projects, or All projects.".into(),
                ));
            }
        }
        self.edit_plugin_choice(row_id, None, Some(scope))
    }

    fn edit_plugin_choice(
        &self,
        row_id: &str,
        on: Option<bool>,
        scope: Option<super::choices::Scope>,
    ) -> crate::Result<()> {
        let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
        let rows = self.plugin_catalog_rows();
        let row = rows
            .iter()
            .find(|row| row.id == row_id)
            .ok_or_else(|| crate::Error::Invalid(format!("No plugin {row_id}")))?;
        let mut choices =
            super::choices::Choices::load(&self.data_dir).map_err(crate::Error::Invalid)?;
        let mut choice = choices.choice(row);
        if let Some(on) = on {
            choice.on = on;
        }
        if let Some(mut scope) = scope {
            if let super::choices::Scope::Projects(paths) = &mut scope {
                paths.sort();
                paths.dedup();
            }
            choice.scope = scope;
        }
        choices.0.insert(row_id.to_owned(), choice);
        choices.save(&self.data_dir).map_err(crate::Error::Invalid)
    }

    /// Completes before replying: Add/Import/Remove need no frontend job runner.
    pub async fn add_library_item(
        &self,
        text: String,
        name: Option<String>,
        project_ids: Option<Vec<String>>,
        values: Option<BTreeMap<String, String>>,
    ) -> crate::Result<()> {
        let projects = self.library_scope_paths(project_ids)?;
        let root = self.data_dir.join("library");
        let data_dir = self.data_dir.clone();
        let home = self.registry().roots().home.clone();
        let registry = self.registry();
        let store = super::launch::CliSecretStore {
            cli: self.brigadier_cli(),
            env: registry.roots().vars.clone(),
        };
        super::super::blocking(move || {
            let mut items = add_parse::parse(&text, name.as_deref(), &home)
                .map_err(|e| crate::Error::Invalid(e.to_string()))?;
            give_values(&mut items, &values.unwrap_or_default()).map_err(crate::Error::Invalid)?;
            if root
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err(crate::Error::Invalid(
                    "The library folder cannot be a symbolic link.".into(),
                ));
            }
            let staging = root.join(format!(".pending-{}", uuid::Uuid::new_v4()));
            let staged_library = staging.join("library");
            let keys = add_plain(&staged_library, &items, projects.as_deref())
                .map_err(crate::Error::Invalid)?;
            let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
            let rows = Catalog::rows(&[brigadier_registry::catalog::library(&staging)]);
            let mut choices =
                super::choices::Choices::load(&data_dir).map_err(crate::Error::Invalid)?;
            for row in &rows {
                // Validate all destinations before publishing any item.
                let file = &row.definitions[0].provenance.file;
                let relative = file
                    .strip_prefix(&staged_library)
                    .map_err(|e| crate::Error::Invalid(e.to_string()))?;
                let destination = root.join(relative);
                if destination.exists() || library_name_exists(&root, &row.name) {
                    return Err(crate::Error::Invalid(format!(
                        "Your Library already has something called {}.",
                        row.name
                    )));
                }
                if keys.contains(&format!("lib:{}", row.name)) {
                    choices.0.insert(
                        row.id.clone(),
                        super::choices::Choice {
                            on: true,
                            scope: projects.clone().map_or(
                                super::choices::Scope::Global(super::choices::Global::Global),
                                super::choices::Scope::Projects,
                            ),
                        },
                    );
                }
            }
            let mut saved = StoredValues::new(&store);
            saved
                .save_items(&items, &staged_library)
                .map_err(crate::Error::Invalid)?;
            choices.save(&data_dir).map_err(crate::Error::Invalid)?;
            for folder in ["skills", "servers", "origins"] {
                let from = staged_library.join(folder);
                if !from.is_dir() {
                    continue;
                }
                let to = root.join(folder);
                if to
                    .symlink_metadata()
                    .is_ok_and(|meta| meta.file_type().is_symlink())
                {
                    return Err(crate::Error::Invalid(
                        "The library folder cannot be a symbolic link.".into(),
                    ));
                }
                std::fs::create_dir_all(&to).map_err(|e| crate::Error::Invalid(e.to_string()))?;
                for entry in
                    std::fs::read_dir(from).map_err(|e| crate::Error::Invalid(e.to_string()))?
                {
                    let entry = entry.map_err(|e| crate::Error::Invalid(e.to_string()))?;
                    let destination = to.join(entry.file_name());
                    if destination.exists() {
                        return Err(crate::Error::Invalid(
                            "The library item already exists.".into(),
                        ));
                    }
                    std::fs::rename(entry.path(), destination)
                        .map_err(|e| crate::Error::Invalid(e.to_string()))?;
                }
            }
            saved.commit();
            let _ = std::fs::remove_dir_all(staging);
            registry.refresh();
            Ok(())
        })
        .await
    }

    fn library_scope_paths(&self, ids: Option<Vec<String>>) -> crate::Result<Option<Vec<PathBuf>>> {
        let Some(ids) = ids else {
            return Ok(None);
        };
        let projects = self.plugin_projects();
        let mut paths = Vec::new();
        for id in ids {
            let found: Vec<_> = projects
                .iter()
                .filter(|(known, _, path)| known == &id || path.to_string_lossy() == id)
                .map(|(_, _, path)| path.clone())
                .collect();
            if found.is_empty() {
                return Err(crate::Error::Invalid(format!("No trusted project {id}")));
            }
            paths.extend(found);
        }
        if paths.is_empty() {
            return Err(crate::Error::Invalid(
                "Pick a project, or All projects.".into(),
            ));
        }
        paths.sort();
        paths.dedup();
        Ok(Some(paths))
    }

    pub async fn import_library_item(
        &self,
        row_id: String,
        name: Option<String>,
        project_ids: Option<Vec<String>>,
    ) -> crate::Result<()> {
        let rows = self.plugin_catalog_rows();
        let row = rows
            .iter()
            .find(|row| row.id == row_id)
            .ok_or_else(|| crate::Error::Invalid("No import offer".into()))?;
        let definition = row
            .definitions
            .iter()
            .find(|definition| definition.provenance.agent.is_none())
            .ok_or_else(|| {
                crate::Error::Invalid("This item is already detected in a CLI.".into())
            })?;
        let brigadier_registry::catalog::Content::Server(server) = &definition.content else {
            return Err(crate::Error::Invalid(
                "Only server imports are supported.".into(),
            ));
        };
        let transport = match &server.transport {
            brigadier_registry::Transport::Stdio {
                command,
                args,
                env,
                cwd,
                ..
            } => TransportSpec::Stdio {
                command: command.clone(),
                args: args.clone(),
                env: env.clone(),
                cwd: cwd.clone(),
            },
            brigadier_registry::Transport::Http { url, headers } => TransportSpec::Http {
                url: url.clone(),
                headers: headers.clone(),
            },
        };
        let spec = ServerSpec {
            name: name.unwrap_or_else(|| server.name.clone()),
            transport,
            oauth: server
                .oauth
                .as_ref()
                .and_then(|oauth| oauth.client_id.as_ref())
                .map(|id| brigadier_registry::item::OAuthSpec {
                    client_id: id.clone(),
                }),
        };
        let values = match &spec.transport {
            TransportSpec::Stdio { env, .. } => env,
            TransportSpec::Http { headers, .. } => headers,
        }
        .iter()
        .map(|(name, value)| (name.clone(), value.expose().to_owned()))
        .collect();
        self.add_library_item(
            server_value(&spec).to_string(),
            None,
            project_ids,
            Some(values),
        )
        .await
    }

    pub async fn remove_library_item(&self, key: String) -> crate::Result<()> {
        let root = self.data_dir.join("library");
        let name = if let Some(name) = key.strip_prefix("lib:") {
            name.to_owned()
        } else {
            let rows = self.plugin_catalog_rows();
            let row = rows
                .iter()
                .find(|row| row.id == key)
                .ok_or_else(|| crate::Error::Invalid("No library item".into()))?;
            if !row
                .definitions
                .iter()
                .any(|definition| definition.provenance.file.starts_with(&root))
            {
                return Err(crate::Error::Invalid(
                    "Detected items can only be turned off, never deleted.".into(),
                ));
            }
            row.definitions
                .iter()
                .find_map(|definition| {
                    definition
                        .provenance
                        .file
                        .strip_prefix(root.join("plugins"))
                        .ok()
                        .and_then(|path| path.components().next())
                        .map(|part| part.as_os_str().to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| row.name.clone())
        };
        let registry = self.registry();
        let store = super::launch::CliSecretStore {
            cli: self.brigadier_cli(),
            env: registry.roots().vars.clone(),
        };
        super::super::blocking(move || {
            let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
            remove_with_secrets(&root, &name, &store).map_err(crate::Error::Invalid)?;
            registry.refresh();
            Ok(())
        })
        .await
    }

    pub fn catalog_item_details(&self, key: String) -> crate::Result<super::PluginItemDetails> {
        use brigadier_registry::catalog::Content;
        let rows = self.plugin_catalog_rows();
        let row = rows
            .iter()
            .find(|row| row.id == key)
            .ok_or_else(|| crate::Error::Invalid("No plugin".into()))?;
        let mut skills = Vec::new();
        let mut server = None;
        for definition in &row.definitions {
            match &definition.content {
                Content::Skill { .. } => {
                    let file = &definition.provenance.file;
                    let path = file.to_string_lossy().into_owned();
                    if skills
                        .iter()
                        .any(|skill: &super::PluginSkillFile| skill.path == path)
                    {
                        continue;
                    }
                    skills.push(super::PluginSkillFile {
                        name: row.name.clone(),
                        path,
                        contents: std::fs::read_to_string(file)
                            .map_err(|e| crate::Error::Invalid(e.to_string()))?,
                        read_only: (!file.starts_with(self.data_dir.join("library/skills"))).then(
                            || "Open this file in your editor to change its instructions.".into(),
                        ),
                    });
                }
                Content::Server(item) if server.is_none() => {
                    server = Some(match &item.transport {
                        brigadier_registry::Transport::Stdio { env, .. } => {
                            super::PluginServerSettings::Stdio {
                                command: item.written.command.clone().unwrap_or_default(),
                                args: item.written.args.clone(),
                                cwd: item.written.cwd.clone(),
                                env_names: env.iter().map(|(name, _)| name.clone()).collect(),
                            }
                        }
                        brigadier_registry::Transport::Http { headers, .. } => {
                            super::PluginServerSettings::Http {
                                url: item.written.url.clone().unwrap_or_default(),
                                header_names: headers
                                    .iter()
                                    .map(|(name, _)| name.clone())
                                    .collect(),
                            }
                        }
                    });
                }
                _ => {}
            }
        }
        Ok(super::PluginItemDetails {
            key,
            kind: if row.kind == brigadier_registry::catalog::Kind::Skill {
                PluginKind::Skill
            } else {
                PluginKind::Server
            },
            skills,
            server,
        })
    }
    pub fn save_catalog_skill(
        &self,
        key: String,
        path: String,
        contents: String,
        expected: String,
    ) -> crate::Result<super::PluginSkillSave> {
        let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
        let row = self.catalog_item_details(key)?;
        let skill = row
            .skills
            .iter()
            .find(|skill| skill.path == path && skill.read_only.is_none())
            .ok_or_else(|| {
                crate::Error::Invalid("Only library skill files can be saved here.".into())
            })?;
        let file = Path::new(&skill.path);
        let canonical =
            std::fs::canonicalize(file).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        let root = std::fs::canonicalize(self.data_dir.join("library/skills"))
            .map_err(|e| crate::Error::Invalid(e.to_string()))?;
        if !canonical.starts_with(root) {
            return Err(crate::Error::Invalid(
                "The skill points outside the library.".into(),
            ));
        }
        if skill.contents != expected {
            return Ok(super::PluginSkillSave::ChangedSinceOpened {
                current: skill.contents.clone(),
            });
        }
        let staging = file.with_file_name(format!(".skill-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&staging, contents).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        std::fs::rename(staging, file).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        self.registry().refresh();
        Ok(super::PluginSkillSave::Saved { job: String::new() })
    }
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;

/// Roll back failed Add/Remove operations without putting credential values in a file.
struct StoredValues<'a> {
    store: &'a dyn brigadier_cli::secrets::Store,
    before: Vec<(String, brigadier_cli::secrets::Values)>,
    committed: bool,
}
impl<'a> StoredValues<'a> {
    fn new(store: &'a dyn brigadier_cli::secrets::Store) -> Self {
        Self {
            store,
            before: Vec::new(),
            committed: false,
        }
    }
    fn replace(
        &mut self,
        id: String,
        values: brigadier_cli::secrets::Values,
    ) -> Result<(), String> {
        let before = self.store.get(&id)?;
        self.before.push((id.clone(), before));
        self.store.put(&id, &values)
    }
    fn save_items(&mut self, items: &[Item], root: &Path) -> Result<(), String> {
        for item in items {
            let Item::Server(spec) = item else {
                continue;
            };
            let pairs = match &spec.transport {
                TransportSpec::Stdio { env, .. } => env,
                TransportSpec::Http { headers, .. } => headers,
            };
            if pairs.is_empty() {
                continue;
            }
            let file = root.join("servers").join(format!("{}.json", spec.name));
            let raw: serde_json::Value = serde_json::from_slice(
                &std::fs::read(file)
                    .map_err(|_| "Couldn't read the staged library reference".to_owned())?,
            )
            .map_err(|_| "Couldn't read the staged library reference".to_owned())?;
            let id = raw
                .get("mcpServers")
                .and_then(|servers| servers.get(&spec.name))
                .and_then(|server| server.pointer("/brigadierSecrets/id"))
                .and_then(serde_json::Value::as_str)
                .ok_or("Missing library credential reference")?;
            self.replace(
                id.to_owned(),
                pairs
                    .iter()
                    .map(|(name, value)| (name.clone(), value.expose().to_owned()))
                    .collect(),
            )?;
        }
        Ok(())
    }
    fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for StoredValues<'_> {
    fn drop(&mut self) {
        if !self.committed {
            for (id, values) in self.before.iter().rev() {
                if self.store.put(id, values).is_err() {
                    tracing::warn!(
                        server = id,
                        "couldn't restore the library credential store after a failed change"
                    );
                }
            }
        }
    }
}

fn library_secret_ids(root: &Path, name: &str) -> Result<Vec<String>, String> {
    valid_library_name(name)?;
    if root
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err("The library folder cannot be a symbolic link.".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let files = [
        root.join("servers").join(format!("{name}.json")),
        root.join("plugins").join(name).join(".mcp.json"),
        root.join("plugins").join(name).join(".codex.mcp.json"),
    ];
    for file in files.iter().filter(|path| path.exists()) {
        let actual = std::fs::canonicalize(file).map_err(|e| e.to_string())?;
        let base = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        if !actual.starts_with(base)
            || file
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err("The library item points outside the library.".into());
        }
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?)
                .map_err(|_| "Couldn't read the library credential reference".to_owned())?;
        let entries = raw
            .get("mcpServers")
            .unwrap_or(&raw)
            .as_object()
            .ok_or("Invalid library server file")?;
        for entry in entries.values() {
            let id = entry
                .pointer("/brigadierSecrets/id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    entry
                        .get("headersHelper")
                        .or_else(|| entry.get("http_headers_helper"))
                        .and_then(serde_json::Value::as_str)
                        .and_then(|helper| helper.split_whitespace().last())
                })
                .or_else(|| {
                    let args = entry.get("args")?.as_array()?;
                    if args.first()?.as_str() == Some("mcp") && args.get(1)?.as_str() == Some("run")
                    {
                        args.get(2)?.as_str()
                    } else {
                        None
                    }
                });
            if let Some(id) = id {
                if id.split_once('/').is_none_or(|(plugin, server)| {
                    let owned_namespace = plugin == name
                        || plugin
                            .strip_prefix(&format!("{name}-"))
                            .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok());
                    !owned_namespace || server.is_empty() || server.contains('/')
                }) {
                    return Err("The credential reference belongs to another library item.".into());
                }
                ids.insert(id.to_owned());
            }
        }
    }
    Ok(ids.into_iter().collect())
}

fn remove_with_secrets(
    root: &Path,
    name: &str,
    store: &dyn brigadier_cli::secrets::Store,
) -> Result<(), String> {
    let ids = library_secret_ids(root, name)?;
    let mut saved = StoredValues::new(store);
    for id in ids {
        saved.replace(id, BTreeMap::new())?;
    }
    remove_plain(root, name)?;
    saved.commit();
    Ok(())
}
