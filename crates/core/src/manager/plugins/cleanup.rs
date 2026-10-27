//! One-time retirement of the old sharing writer (L9, L10). Never infer ownership
//! from a matching name outside the reserved @brigadier namespace.
use std::collections::BTreeSet;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;
use brigadier_registry::Secret;
use brigadier_registry::adapter::{CliCall, Place, Step, adapter};
use brigadier_registry::claude_config;
use brigadier_registry::item::{ServerSpec, TransportSpec};
use brigadier_registry::{Agent, Catalog};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use super::cleanup_context::Ctx;
use super::cleanup_state::Sharing;

const CHECKPOINT: &str = "cleanup.json";

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Checkpoint {
    completed: BTreeSet<u8>,
    done: bool,
    // Discovered before deleting install records; kept across interrupted runs.
    projects: BTreeSet<PathBuf>,
    restored: BTreeSet<String>,
    marketplaces: BTreeSet<String>,
    errors: Vec<String>,
}

pub(super) fn finished(data: &Path) -> bool {
    read_json::<Checkpoint>(&data.join(CHECKPOINT))
        .ok()
        .flatten()
        .is_some_and(|state| state.done)
}

impl super::super::SessionManager {
    pub(crate) async fn cleanup_plugins(&self) -> Result<(), String> {
        // Startup tests have a separate temp-home entry point below. Never consult the
        // test process's real default home or start any installed CLI from a test.
        if cfg!(test) {
            return Ok(());
        }
        let overridden = std::env::var_os("BRIGADIER_DATA_DIR").is_some();
        let installed = brigadier_sandbox::default_data_dir().map_err(|e| e.to_string())?;
        if overridden || self.data_dir != installed {
            return Ok(());
        }
        if finished(&self.data_dir) {
            return Ok(());
        }
        let data = self.data_dir.clone();
        let mut setup = self.plugin_setup();
        // Previously trusted projects can still contain the old writer's entries.
        setup.projects = self
            .core
            .catalog()
            .projects
            .iter()
            .flat_map(|project| project.repos.iter().map(|repo| PathBuf::from(&repo.path)))
            .collect();
        let codex_homes = self.runtime.account_homes(ProviderKind::Codex);
        let result = tokio::task::spawn_blocking(move || {
            super::cleanup_context::with_ctx(&setup, |ctx| {
                let import = |old: &Sharing| {
                    let mut projects = sharing_projects(old)?;
                    projects.extend(ctx.projects.iter().cloned());
                    // Import rows which exist only in recorded worktrees before their
                    // install records are removed, including unregistered projects.
                    for dir in claude_dirs(ctx) {
                        let file = dir.join("plugins/installed_plugins.json");
                        if let Some(text) = read_text(&file)? {
                            let value = Json::parse(&text)
                                .and_then(|json| json.value(&text))
                                .map_err(|e| format!("{}: {e}", file.display()))?;
                            projects.extend(install_projects(ctx, &value)?);
                        }
                    }
                    let projects: Vec<PathBuf> = projects.into_iter().collect();
                    let rows = Catalog::read_rows(ctx.roots, &projects);
                    super::choices::import_sharing_file(
                        &data,
                        &ctx.library.root().join(".sharing.json"),
                        &rows,
                    )
                    .map(|_| ())
                };
                let runner = |agent, home: &Path, steps: &[Step]| {
                    let mut roots = ctx.roots.clone();
                    match agent {
                        Agent::Claude => {
                            roots.claude_dir = home.to_owned();
                            roots.claude_json = claude_config::config_file(&roots.home, Some(home));
                        }
                        Agent::Codex => roots.codex_home = home.to_owned(),
                    }
                    let binary = (ctx.binary)(agent);
                    brigadier_registry::adapter::apply(
                        adapter(agent),
                        &roots,
                        &Catalog::read(&roots, None),
                        steps,
                        ctx.env,
                        binary.as_deref(),
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                };
                Cleanup {
                    data: &data,
                    installed_default: &installed,
                    overridden,
                    ctx,
                    codex_homes: &codex_homes,
                }
                .run(&import, &runner)
            })
        })
        .await
        .map_err(|e| e.to_string())?;
        if let Err(error) = &result {
            tracing::warn!(%error, "plugin cleanup is incomplete; retrying next start");
        }
        result
    }
}

/// The importer must merge existing choices and sync plugins.json before returning.
/// This seam lets part A supply its Sharing -> Choices translation without coupling
/// cleanup's checkpoints to the choices schema.
pub(super) type Import<'a> = &'a dyn Fn(&Sharing) -> Result<(), String>;
pub(super) type Runner<'a> = &'a dyn Fn(Agent, &Path, &[Step]) -> Result<(), String>;

pub(super) struct Cleanup<'a> {
    pub data: &'a Path,
    pub installed_default: &'a Path,
    pub overridden: bool,
    pub ctx: &'a Ctx<'a>,
    pub codex_homes: &'a [PathBuf],
}

impl Cleanup<'_> {
    /// An error leaves this step uncheckpointed. Other files in that step are still
    /// processed; next launch retries it. The old sharing record is never erased.
    pub(super) fn run(&self, import: Import<'_>, runner: Runner<'_>) -> Result<(), String> {
        if self.overridden || self.data != self.installed_default {
            return Ok(());
        }
        std::fs::create_dir_all(self.data).map_err(|e| e.to_string())?;
        // Serialize daemon starts without accepting an existing/stale lock as success.
        let _lock = CleanupLock::take(self.data)?;
        let file = self.data.join(CHECKPOINT);
        let mut state = read_json::<Checkpoint>(&file)?.unwrap_or_default();
        if state.done {
            return Ok(());
        }
        // Record the attempted upgrade before reading old state, so even a corrupt
        // sharing file cannot allow a later callback to restart the old writer.
        save(&file, &state)?;
        let sharing = match read_json::<Sharing>(&self.ctx.library.root().join(".sharing.json")) {
            Ok(sharing) => sharing,
            Err(error) => {
                state.errors = vec![error.clone()];
                save(&file, &state)?;
                return Err(error);
            }
        };
        if let Some(old) = &sharing {
            state.projects.extend(sharing_projects(old)?);
        }
        for step in 1..=5 {
            if state.completed.contains(&step) || (sharing.is_none() && step != 4) {
                continue;
            }
            state.errors.clear();
            let result = match step {
                1 => import(sharing.as_ref().unwrap()),
                2 => self.keep_library(sharing.as_ref().unwrap()),
                3 => self.restore(sharing.as_ref().unwrap(), &mut state, runner),
                4 => self.namespace(&mut state, runner),
                // Old Sharing has no created-file or exclusion ownership field. Even
                // a Brigadier-only file could predate its install; retain exclusions.
                5 => Ok(()),
                _ => unreachable!(),
            };
            match result {
                Ok(()) => {
                    state.completed.insert(step);
                }
                Err(error) => {
                    state.errors.push(error.clone());
                    save(&file, &state)?;
                    return Err(error);
                }
            }
            save(&file, &state)?;
        }
        state.done = true;
        save(&file, &state)
    }

    fn keep_library(&self, sharing: &Sharing) -> Result<(), String> {
        let source = self.ctx.library.root();
        let target = self.data.join("library");
        for dir in [
            &target,
            &target.join("plugins"),
            &target.join(".claude-plugin"),
        ] {
            if std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(format!(
                    "{}: destination is a symlink; left untouched",
                    dir.display()
                ));
            }
        }
        if source == target {
            return Ok(());
        }
        // Keep complete packages, including their manifests and secret-reference metadata.
        // CLI marketplace removal can remove its caches, but never this durable copy.
        let manifest = source.join(".claude-plugin/marketplace.json");
        let Some(value) = read_json::<Value>(&manifest)? else {
            return Ok(());
        };
        let entries = value
            .get("plugins")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{}: plugins is not an array", manifest.display()))?;
        for entry in entries {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{}: plugin has no name", manifest.display()))?;
            if sharing.items.contains_key(name) {
                continue;
            }
            let mut components = Path::new(name).components();
            if !matches!(components.next(), Some(std::path::Component::Normal(_)))
                || components.next().is_some()
            {
                return Err(format!("{}: unsafe plugin name", manifest.display()));
            }
            copy_tree(
                &source.join("plugins").join(name),
                &target.join("plugins").join(name),
            )?;
        }
        // The catalog still reads this manifest during the transition. It is copied
        // byte-for-byte; no old package or user original is deleted here.
        copy_tree(&manifest, &target.join(".claude-plugin/marketplace.json"))
    }

    fn restore(
        &self,
        sharing: &Sharing,
        state: &mut Checkpoint,
        runner: Runner<'_>,
    ) -> Result<(), String> {
        let value = serde_json::to_value(sharing).map_err(|e| e.to_string())?;
        let mut flags = Vec::new();
        // Undo an interrupted flag transaction first, then restore its recorded baseline.
        if let Some(pending) = value.get("pending").and_then(|p| p.get("flags")) {
            flags.extend(
                serde_json::from_value::<Vec<Flag>>(pending.clone()).map_err(|e| e.to_string())?,
            );
        }
        flags.extend(
            serde_json::from_value::<Vec<Flag>>(value["owned"].clone())
                .map_err(|e| e.to_string())?,
        );
        let mut errors = Vec::new();
        for flag in flags {
            collect(&mut errors, flag.restore());
        }
        let moved = self.ctx.library.root().join(".moved");
        if moved.exists() {
            let files = std::fs::read_dir(&moved).map_err(|e| e.to_string())?;
            for file in files {
                let file = file.map_err(|e| e.to_string())?.path();
                if file.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let result = (|| {
                    let Some(moved) = read_json::<Moved>(&file)? else {
                        return Ok(());
                    };
                    for (i, original) in moved.originals.iter().enumerate() {
                        let key = format!("{}:{i}", file.display());
                        if moved.restored.contains(&i) || state.restored.contains(&key) {
                            continue;
                        }
                        original.restore(self.ctx, runner)?;
                        state.restored.insert(key);
                        save(&self.data.join(CHECKPOINT), state)?;
                    }
                    Ok(())
                })();
                collect(&mut errors, result);
            }
        }
        join_errors(errors)
    }

    fn namespace(&self, state: &mut Checkpoint, runner: Runner<'_>) -> Result<(), String> {
        let mut errors = Vec::new();
        state.projects.extend(self.ctx.projects.iter().cloned());
        // Every recorded install path is used literally, including linked worktrees.
        let claude_dirs = claude_dirs(self.ctx);
        let mut installed = Vec::new();
        for dir in &claude_dirs {
            let file = dir.join("plugins/installed_plugins.json");
            let result: Result<(), String> = (|| {
                let Some(text) = read_text(&file)? else {
                    return Ok(());
                };
                let value = Json::parse(&text)?.value(&text)?;
                state.projects.extend(install_projects(self.ctx, &value)?);
                installed.push(file.clone());
                Ok(())
            })();
            collect(
                &mut errors,
                result.map_err(|e| format!("{}: {e}", file.display())),
            );
        }
        // Persist discovered paths before any file or CLI can remove the install records.
        save(&self.data.join(CHECKPOINT), state)?;
        let mut codex_dirs = BTreeSet::from([self.ctx.roots.codex_home.clone()]);
        codex_dirs.extend(self.codex_homes.iter().cloned());
        let mut codex: BTreeSet<PathBuf> = codex_dirs
            .iter()
            .map(|dir| dir.join("config.toml"))
            .collect();
        for project in &state.projects {
            codex.insert(project.join(".codex/config.toml"));
            for name in ["settings.json", "settings.local.json"] {
                let file = project.join(".claude").join(name);
                collect(&mut errors, clean_json(&file, "enabledPlugins"));
            }
        }
        for dir in &claude_dirs {
            // User-scoped switches can also have been written by the old CLI install.
            for name in ["settings.json", "settings.local.json"] {
                collect(&mut errors, clean_json(&dir.join(name), "enabledPlugins"));
            }
        }
        for file in &codex {
            // The CLI needs its registration to remove its own marketplace. Project
            // registrations can be edited directly; user registration goes last.
            collect(
                &mut errors,
                clean_toml(
                    file,
                    !codex_dirs
                        .iter()
                        .any(|dir| file == &dir.join("config.toml")),
                ),
            );
        }
        for file in installed {
            collect(&mut errors, clean_json(&file, "plugins"));
        }
        // Never let a CLI rewrite a config which failed parsing. It will be retried.
        join_errors(errors)?;
        for (agent, dirs) in [
            (Agent::Claude, claude_dirs),
            (Agent::Codex, codex_dirs.clone()),
        ] {
            for dir in dirs {
                let key = format!("{agent:?}:{}", dir.display());
                if state.marketplaces.contains(&key) {
                    continue;
                }
                // Only call remove when its catalog actually lists our marketplace.
                let listed = match agent {
                    Agent::Claude => {
                        let file = dir.join("plugins/known_marketplaces.json");
                        read_json::<Value>(&file)?.is_some_and(|v| v.get("brigadier").is_some())
                    }
                    Agent::Codex => {
                        let file = dir.join("config.toml");
                        match read_text(&file)? {
                            Some(text) => {
                                let doc: DocumentMut = text
                                    .parse()
                                    .map_err(|_| format!("{}: invalid TOML", file.display()))?;
                                doc.get("marketplaces")
                                    .and_then(|m| m.get("brigadier"))
                                    .is_some()
                            }
                            None => false,
                        }
                    }
                };
                if listed {
                    let args = vec![
                        "plugin".into(),
                        "marketplace".into(),
                        "remove".into(),
                        "brigadier".into(),
                    ];
                    let call = CliCall {
                        program: if agent == Agent::Claude {
                            "claude"
                        } else {
                            "codex"
                        },
                        args,
                        cwd: None,
                        needs_terminal: false,
                    };
                    // The runner receives the effective home explicitly in a separate
                    // Run call environment through the production adapter context.
                    runner(agent, &dir, &[Step::Run(call)])?;
                }
                state.marketplaces.insert(key);
                save(&self.data.join(CHECKPOINT), state)?;
            }
        }
        let mut errors = Vec::new();
        for dir in codex_dirs {
            collect(&mut errors, clean_toml(&dir.join("config.toml"), true));
        }
        join_errors(errors)
    }
}

fn sharing_projects(old: &Sharing) -> Result<BTreeSet<PathBuf>, String> {
    let value = serde_json::to_value(old).map_err(|e| e.to_string())?;
    let mut paths = BTreeSet::new();
    for choice in value["chosen"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, choice)| &choice["scope"])
        .chain(
            value["offScopes"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(_, scope)| scope),
        )
    {
        paths.extend(
            choice
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from),
        );
    }
    for flag in value["owned"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(value["pending"]["flags"].as_array().into_iter().flatten())
    {
        if let Some(folder) = flag["folder"].as_str() {
            paths.insert(PathBuf::from(folder));
        }
    }
    Ok(paths)
}

fn claude_dirs(ctx: &Ctx<'_>) -> BTreeSet<PathBuf> {
    let mut dirs = BTreeSet::from([ctx.roots.claude_dir.clone()]);
    dirs.extend(
        ctx.claude_configs
            .iter()
            .filter_map(|file| file.parent().map(Path::to_owned)),
    );
    dirs
}

fn install_projects(ctx: &Ctx<'_>, value: &Value) -> Result<BTreeSet<PathBuf>, String> {
    let plugins = value
        .get("plugins")
        .and_then(Value::as_object)
        .ok_or("plugins is not an object")?;
    let mut projects = BTreeSet::new();
    for (id, records) in plugins {
        if !ours(id) {
            continue;
        }
        let records = records
            .as_array()
            .ok_or("install records are not an array")?;
        for record in records {
            if let Some(path) = record.get("projectPath").and_then(Value::as_str) {
                projects.insert(if let Some(rest) = path.strip_prefix("~/") {
                    ctx.roots.home.join(rest)
                } else {
                    ctx.roots.home.join(path)
                });
            }
        }
    }
    Ok(projects)
}

#[derive(Deserialize)]
struct Flag {
    file: PathBuf,
    folder: Option<PathBuf>,
    name: String,
    claude: bool,
    before: Option<bool>,
    after: Option<bool>,
}
impl Flag {
    fn restore(&self) -> Result<(), String> {
        if self.claude {
            let folder = self
                .folder
                .as_deref()
                .ok_or("recorded Claude flag has no project")?;
            // Same conditional restoration as flags::restore: a later user edit wins.
            let current = claude_config::project_settings(&self.file, folder)?
                .off
                .contains(&self.name);
            if Some(current) == self.after {
                claude_config::set_off_flags(
                    &self.file,
                    &[claude_config::OffFlag {
                        folder: folder.to_owned(),
                        name: self.name.clone(),
                        off: self.before == Some(true),
                    }],
                    std::time::Duration::ZERO,
                )?;
            }
            return Ok(());
        }
        edit_file(&self.file, |text| {
            let mut doc: DocumentMut = text.parse().map_err(|_| "invalid TOML".to_owned())?;
            let Some(server) = doc
                .get_mut("mcp_servers")
                .and_then(|s| s.get_mut(&self.name))
            else {
                return Ok(None);
            };
            if server.get("enabled").and_then(Item::as_bool) != self.after {
                return Ok(None);
            }
            if let Some(before) = self.before {
                let decor = server
                    .get("enabled")
                    .and_then(Item::as_value)
                    .map(|value| value.decor().clone());
                let mut restored = toml_edit::Value::from(before);
                if let Some(decor) = decor {
                    *restored.decor_mut() = decor;
                }
                server["enabled"] = Item::Value(restored);
            } else if let Some(table) = server.as_table_like_mut() {
                table.remove("enabled");
            }
            Ok(Some(doc.to_string()))
        })
    }
}

fn ours(name: &str) -> bool {
    name.ends_with("@brigadier")
}
fn collect(errors: &mut Vec<String>, result: Result<(), String>) {
    if let Err(e) = result {
        errors.push(e);
    }
}
fn join_errors(errors: Vec<String>) -> Result<(), String> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
fn read_text(file: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}
fn read_json<T: serde::de::DeserializeOwned>(file: &Path) -> Result<Option<T>, String> {
    read_text(file)?
        .map(|s| serde_json::from_str(&s).map_err(|e| format!("{}: {e}", file.display())))
        .transpose()
}
fn save(file: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let temp = file.with_extension(format!("{}.tmp", std::process::id()));
    (|| {
        let mut out = std::fs::File::create(&temp)?;
        out.write_all(&text)?;
        out.sync_all()?;
        std::fs::rename(&temp, file)?;
        std::fs::File::open(file.parent().unwrap())?.sync_all()
    })()
    .map_err(|e| format!("{}: {e}", file.display()))
}

/// Use the same lock and compare-before-rename protection as the old flag writer.
fn edit_file(
    file: &Path,
    mut edit: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<(), String> {
    match std::fs::metadata(file) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    }
    let target = std::fs::canonicalize(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let file = target.as_path();
    claude_config::rewrite_locked(file, |before| {
        let after = edit(before)?;
        if after.as_deref().is_some_and(|text| text.trim().is_empty()) {
            // Called under the CLI-compatible lock; compare immediately before unlink.
            if std::fs::read_to_string(file).map_err(|e| e.to_string())? != before {
                return Err("settings changed before removal; retry next start".into());
            }
            std::fs::remove_file(file).map_err(|e| e.to_string())?;
            std::fs::File::open(file.parent().unwrap())
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
            return Ok(None);
        }
        Ok(after)
    })
    .map(|_| ())
    .map_err(|e| format!("{}: {e}", file.display()))
}

fn clean_toml(file: &Path, remove_marketplace: bool) -> Result<(), String> {
    edit_file(file, |text| {
        let mut doc: DocumentMut = text.parse().map_err(|_| "invalid TOML".to_owned())?;
        let mut changed = false;
        for section in ["plugins", "marketplaces"] {
            if section == "marketplaces" && !remove_marketplace {
                continue;
            }
            if let Some(table) = doc.get_mut(section).and_then(Item::as_table_like_mut) {
                let keys: Vec<String> = table
                    .iter()
                    .filter(|(key, _)| {
                        if section == "plugins" {
                            ours(key)
                        } else {
                            *key == "brigadier"
                        }
                    })
                    .map(|(key, _)| key.to_owned())
                    .collect();
                for key in keys {
                    table.remove(&key);
                    changed = true;
                }
                if table.is_empty() {
                    doc.remove(section);
                }
            }
        }
        Ok(changed.then(|| keep_toml_comments(text, doc.to_string())))
    })
}

// toml_edit keeps the decoration of retained entries. Preserve comments attached
// to removed entries too: a comment alone is enough reason to keep the file.
fn keep_toml_comments(before: &str, mut after: String) -> String {
    let mut remaining = toml_comments(&after);
    for comment in toml_comments(before) {
        if let Some(index) = remaining.iter().position(|known| *known == comment) {
            remaining.remove(index);
        } else {
            if !after.is_empty() && !after.ends_with('\n') {
                after.push('\n');
            }
            after.push_str(&comment);
            after.push('\n');
        }
    }
    after
}

fn toml_comments(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut quote: Option<(u8, bool)> = None;
    let mut comments = Vec::new();
    while at < bytes.len() {
        if let Some((delimiter, multiline)) = quote {
            if delimiter == b'"' && bytes[at] == b'\\' {
                at += 2;
                continue;
            }
            if bytes[at] == delimiter {
                if !multiline {
                    quote = None;
                } else if bytes[at..].starts_with(&[delimiter; 3]) {
                    quote = None;
                    at += 2;
                }
            }
        } else if bytes[at] == b'#' {
            let start = at;
            while at < bytes.len() && !matches!(bytes[at], b'\r' | b'\n') {
                at += 1;
            }
            comments.push(text[start..at].to_owned());
            continue;
        } else if matches!(bytes[at], b'"' | b'\'') {
            let delimiter = bytes[at];
            let multiline = bytes[at..].starts_with(&[delimiter; 3]);
            quote = Some((delimiter, multiline));
            if multiline {
                at += 2;
            }
        }
        at += 1;
    }
    comments
}

fn clean_json(file: &Path, section: &str) -> Result<(), String> {
    edit_file(file, |text| {
        let json = Json::parse(text)?;
        let Some(holder) = json.child(section) else {
            return Ok(None);
        };
        let members = holder.members()?;
        let removed: Vec<usize> = members
            .iter()
            .enumerate()
            .filter(|(_, m)| ours(&m.key))
            .map(|(i, _)| i)
            .collect();
        if removed.is_empty() {
            return Ok(None);
        }
        let mut edits = Vec::new();
        if removed.len() == members.len() && section == "enabledPlugins" {
            json.remove_members(&[section], &mut edits)?;
        } else {
            holder.remove_indices(&removed, &mut edits)?;
        }
        let mut after = text.to_owned();
        edits.sort_by_key(|r| r.start);
        for range in edits.into_iter().rev() {
            let removed = &text[range.clone()];
            let comments: String = json_comment_spans(removed)?
                .into_iter()
                .map(|span| &removed[span])
                .collect();
            after.replace_range(range, &comments);
        }
        let remaining = Json::parse(&after)?;
        if remaining.members()?.is_empty() && mask_comments(&after)? == after {
            after.clear();
        }
        Ok(Some(after))
    })
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    let result = (|| -> std::io::Result<()> {
        let meta = std::fs::symlink_metadata(source)?;
        if !meta.file_type().is_symlink()
            && std::fs::symlink_metadata(target).is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err(std::io::Error::other(
                "destination is a symlink; left untouched",
            ));
        }
        if meta.file_type().is_symlink() {
            // Packages may contain links to external user files. Preserve the link,
            // never traverse it or remove its target.
            let link = std::fs::read_link(source)?;
            if let Ok(existing) = std::fs::read_link(target)
                && existing == link
            {
                return Ok(());
            }
            std::fs::create_dir_all(target.parent().unwrap())?;
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(link, target)?;
            }
            #[cfg(not(unix))]
            {
                return Err(std::io::Error::other(
                    "cannot migrate a library symlink on this platform",
                ));
            }
        } else if meta.is_dir() {
            std::fs::create_dir_all(target)?;
            for entry in std::fs::read_dir(source)? {
                let entry = entry?;
                copy_tree(&entry.path(), &target.join(entry.file_name()))
                    .map_err(std::io::Error::other)?;
            }
        } else if target.exists() {
            if std::fs::read(source)? != std::fs::read(target)? {
                return Err(std::io::Error::other("destination differs; left untouched"));
            }
        } else {
            std::fs::create_dir_all(target.parent().unwrap())?;
            let mut input = std::fs::File::open(source)?;
            let temp = target.with_extension(format!("cleanup-{}.tmp", uuid::Uuid::new_v4()));
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            std::io::copy(&mut input, &mut out)?;
            out.set_permissions(meta.permissions())?;
            out.sync_all()?;
            std::fs::rename(temp, target)?;
            std::fs::File::open(target.parent().unwrap())?.sync_all()?;
        }
        Ok(())
    })();
    result.map_err(|e| format!("{} -> {}: {e}", source.display(), target.display()))
}

struct CleanupLock(std::fs::File);
impl CleanupLock {
    fn take(data: &Path) -> Result<Self, String> {
        // OS lock is released on a killed process, unlike a persistent create_new lock.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(data.join("cleanup.lock"))
            .map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|e| format!("cleanup is already running: {e}"))?;
        Ok(Self(file))
    }
}
impl Drop for CleanupLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Moved {
    pub originals: Vec<Original>,
    /// Which of them a Remove has put back already, by index.
    #[serde(default)]
    pub restored: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Original {
    /// An agent's own server: put back by its own commands. Its values are in the Keychain,
    /// under `secrets`.
    Server {
        agent: ProviderKind,
        name: String,
        /// Its project, for one kept to a project.
        project: Option<PathBuf>,
        transport: SavedTransport,
        secrets: String,
    },
    /// A skill's folder (or link), moved aside to `kept`.
    Skill { from: PathBuf, kept: PathBuf },
}

/// A server's transport without its values: their names only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum SavedTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        headers: Vec<String>,
    },
}

impl Original {
    /// Already where it was: a skill's folder back, its kept copy gone.
    fn back(&self) -> bool {
        match self {
            Self::Skill { from, kept } => {
                from.symlink_metadata().is_ok()
                    && kept
                        .symlink_metadata()
                        .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
            }
            Self::Server { .. } => false,
        }
    }

    /// Puts it back as it was (a server's values stay kept: [`Self::secrets`]).
    fn restore(&self, ctx: &Ctx<'_>, runner: Runner<'_>) -> Result<(), String> {
        match self {
            Self::Skill { from, kept } => {
                if self.back() {
                    return Ok(());
                }
                if from.symlink_metadata().is_ok() {
                    return Err(format!(
                        "Couldn't put {} back: something is already there.",
                        from.display()
                    ));
                }
                if let Some(dir) = from.parent() {
                    std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
                }
                std::fs::rename(kept, from)
                    .and_then(|()| std::fs::File::open(from.parent().unwrap())?.sync_all())
                    .and_then(|()| std::fs::File::open(kept.parent().unwrap())?.sync_all())
                    .map_err(|err| format!("Couldn't put {} back: {err}.", from.display()))
            }
            Self::Server {
                agent,
                name,
                project,
                transport,
                secrets,
            } => {
                let mut values = ctx.secrets.load(secrets)?;
                let mut take = |names: &[String]| -> Result<Vec<(String, Secret)>, String> {
                    names.iter().map(|name| {
                        let value = values.remove(name).ok_or_else(|| format!("Couldn't put the server back: its saved value {name} is missing."))?;
                        Ok((name.clone(), Secret::new(value)))
                    }).collect()
                };
                let transport = match transport {
                    SavedTransport::Stdio {
                        command,
                        args,
                        env,
                        cwd,
                    } => TransportSpec::Stdio {
                        command: command.clone(),
                        args: args.clone(),
                        env: take(env)?,
                        cwd: cwd.clone(),
                    },
                    SavedTransport::Http { url, headers } => TransportSpec::Http {
                        url: url.clone(),
                        headers: take(headers)?,
                    },
                };
                let spec = ServerSpec {
                    name: name.clone(),
                    transport,
                    oauth: None,
                };
                let place = project
                    .as_deref()
                    .map_or(Place::Everywhere, |path| Place::Project {
                        path: path.to_owned(),
                        shared: false,
                    });
                let agent = adapter(
                    super::ops::agent_of(*agent)
                        .ok_or("Brigadier no longer drives the agent it came from.")?,
                );
                // A crash after the CLI restored it but before the checkpoint must be safe
                // to retry. An independently changed entry is never overwritten.
                let catalog = Catalog::read(ctx.roots, project.as_deref());
                if let Some(existing) =
                    catalog
                        .servers
                        .iter()
                        .chain(&catalog.off.servers)
                        .find(|server| {
                            server.name == *name
                                && server.project == *project
                                && server.plugin.is_none()
                                && agent.owns(server.source)
                        })
                {
                    let same_values = |a: &[(String, Secret)], b: &[(String, Secret)]| {
                        a.len() == b.len() && a.iter().all(|value| b.contains(value))
                    };
                    let same = match (&spec.transport, &existing.transport) {
                        (
                            TransportSpec::Stdio {
                                command,
                                args,
                                env,
                                cwd,
                            },
                            brigadier_registry::Transport::Stdio {
                                command: other,
                                args: other_args,
                                env: other_env,
                                cwd: other_cwd,
                                ..
                            },
                        ) => {
                            command == existing.written.command.as_ref().unwrap_or(other)
                                && args == other_args
                                && cwd == other_cwd
                                && same_values(env, other_env)
                        }
                        (
                            TransportSpec::Http { url, headers },
                            brigadier_registry::Transport::Http {
                                url: other,
                                headers: other_headers,
                            },
                        ) => url == other && same_values(headers, other_headers),
                        _ => false,
                    };
                    if !(same && existing.oauth.is_none()) {
                        // A user's replacement wins. Keep the saved original in .moved,
                        // but do not let a safe non-overwrite block retiring our installs.
                        tracing::info!(
                            "kept a user replacement of a legacy MCP server; saved original retained"
                        );
                    }
                    return Ok(());
                }
                let steps = agent.add_server(name, &spec, &place).map_err(|err| err.0)?;
                runner(
                    agent.agent(),
                    match agent.agent() {
                        Agent::Claude => &ctx.roots.claude_dir,
                        Agent::Codex => &ctx.roots.codex_home,
                    },
                    &steps,
                )?;
                Ok(())
            }
        }
    }
}

/// JSON spans, so removing a member never reserializes a neighbour. Comments and
/// trailing commas are accepted and retained, as in the CLI's JSONC settings.
struct Json {
    span: Range<usize>,
    object: Option<Vec<Member>>,
    commas: Vec<usize>,
}
struct Member {
    key: String,
    start: usize,
    value: Json,
}
impl Json {
    fn parse(text: &str) -> Result<Self, String> {
        let masked = mask_json(text)?;
        serde_json::from_str::<Value>(&masked).map_err(|e| format!("invalid JSON: {e}"))?;
        let mut at = 0;
        let node = Self::node(text, &mask_comments(text)?, &mut at)?;
        node.members()?;
        Ok(node)
    }
    fn value(&self, text: &str) -> Result<Value, String> {
        serde_json::from_str(&mask_json(&text[self.span.clone()])?).map_err(|e| e.to_string())
    }
    fn members(&self) -> Result<&[Member], String> {
        self.object
            .as_deref()
            .ok_or_else(|| "expected a JSON object".into())
    }
    fn child(&self, key: &str) -> Option<&Json> {
        self.object
            .as_ref()?
            .iter()
            .find(|m| m.key == key)
            .map(|m| &m.value)
    }
    fn remove_members(&self, keys: &[&str], out: &mut Vec<Range<usize>>) -> Result<(), String> {
        let indices = self
            .members()?
            .iter()
            .enumerate()
            .filter(|(_, m)| keys.contains(&m.key.as_str()))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        self.remove_indices(&indices, out)
    }
    fn remove_indices(&self, indices: &[usize], out: &mut Vec<Range<usize>>) -> Result<(), String> {
        let members = self.members()?;
        for i in indices {
            out.push(members[*i].start..members[*i].value.span.end);
        }
        for (i, comma) in self.commas.iter().enumerate() {
            if indices.contains(&i) || !(i + 1..members.len()).any(|next| !indices.contains(&next))
            {
                out.push(*comma..*comma + 1);
            }
        }
        Ok(())
    }
    fn node(text: &str, masked: &str, at: &mut usize) -> Result<Self, String> {
        let bytes = masked.as_bytes();
        whitespace(bytes, at);
        let start = *at;
        let mut object = None;
        let mut commas = Vec::new();
        match bytes.get(*at) {
            Some(b'{') => {
                *at += 1;
                whitespace(bytes, at);
                let mut members = Vec::new();
                while bytes.get(*at) != Some(&b'}') {
                    let key_start = *at;
                    string_end(bytes, at)?;
                    let key: String =
                        serde_json::from_str(&text[key_start..*at]).map_err(|e| e.to_string())?;
                    if members.iter().any(|m: &Member| m.key == key) {
                        return Err("duplicate JSON key; left untouched".into());
                    }
                    whitespace(bytes, at);
                    *at += 1; // colon, already validated by serde_json
                    let value = Self::node(text, masked, at)?;
                    members.push(Member {
                        key,
                        start: key_start,
                        value,
                    });
                    whitespace(bytes, at);
                    if bytes.get(*at) == Some(&b',') {
                        commas.push(*at);
                        *at += 1;
                        whitespace(bytes, at);
                    } else {
                        break;
                    }
                }
                *at += 1;
                object = Some(members);
            }
            Some(b'[') => {
                *at += 1;
                whitespace(bytes, at);
                while bytes.get(*at) != Some(&b']') {
                    Self::node(text, masked, at)?;
                    whitespace(bytes, at);
                    if bytes.get(*at) == Some(&b',') {
                        *at += 1;
                        whitespace(bytes, at);
                    } else {
                        break;
                    }
                }
                *at += 1;
            }
            Some(b'"') => string_end(bytes, at)?,
            Some(_) => {
                while bytes
                    .get(*at)
                    .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(b))
                {
                    *at += 1;
                }
            }
            None => return Err("unexpected end of JSON".into()),
        }
        Ok(Self {
            span: start..*at,
            object,
            commas,
        })
    }
}
fn whitespace(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).is_some_and(u8::is_ascii_whitespace) {
        *at += 1;
    }
}
fn string_end(bytes: &[u8], at: &mut usize) -> Result<(), String> {
    if bytes.get(*at) != Some(&b'"') {
        return Err("expected a JSON string".into());
    }
    *at += 1;
    while let Some(b) = bytes.get(*at) {
        *at += 1;
        match b {
            b'\\' => *at += 1,
            b'"' => return Ok(()),
            _ => {}
        }
    }
    Err("unterminated JSON string".into())
}
fn json_comment_spans(text: &str) -> Result<Vec<Range<usize>>, String> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'"' {
            string_end(bytes, &mut at)?;
            continue;
        }
        let start = at;
        if bytes[at..].starts_with(b"//") {
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            if at < bytes.len() {
                at += 1;
            }
            spans.push(start..at);
        } else if bytes[at..].starts_with(b"/*") {
            at += 2;
            while at < bytes.len() && !bytes[at..].starts_with(b"*/") {
                at += 1;
            }
            if at + 1 >= bytes.len() {
                return Err("unterminated JSON comment".into());
            }
            at += 2;
            spans.push(start..at);
        } else {
            at += 1;
        }
    }
    Ok(spans)
}
fn mask_comments(text: &str) -> Result<String, String> {
    let mut bytes = text.as_bytes().to_vec();
    for span in json_comment_spans(text)? {
        for byte in &mut bytes[span] {
            if !matches!(*byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}
fn mask_json(text: &str) -> Result<String, String> {
    let mut bytes = mask_comments(text)?.into_bytes();
    // Validate JSONC with serde_json, keeping offsets identical to the original text.
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'"' {
            string_end(&bytes, &mut at)?;
            continue;
        }
        if bytes[at] == b',' {
            let mut next = at + 1;
            whitespace(&bytes, &mut next);
            if matches!(bytes.get(next), Some(b'}' | b']')) {
                bytes[at] = b' ';
            }
        }
        at += 1;
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "cleanup_tests.rs"]
mod tests;
