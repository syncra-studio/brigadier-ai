//! Copy + native off flags. Intent is durable before an agent file changes.
use super::*;
use brigadier_registry::Source;
use brigadier_registry::adapter::{Step, TomlEdit, check_steps};
use brigadier_registry::claude_config::{self, Approval, OffFlag};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Choice {
    pub on: bool,
    pub scope: Option<Vec<PathBuf>>,
}
impl Choice {
    fn at(&self, path: &Path) -> bool {
        self.on
            && self
                .scope
                .as_ref()
                .is_none_or(|paths| paths.iter().any(|p| p == path))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Flag {
    pub owner: String,
    pub file: PathBuf,
    /// Claude's key; None for Codex's user config.
    pub folder: Option<PathBuf>,
    pub name: String,
    pub claude: bool,
    pub copy: bool,
    pub before: Option<bool>,
    pub after: Option<bool>,
}
impl Flag {
    fn same(&self, other: &Self) -> bool {
        self.file == other.file
            && self.folder == other.folder
            && self.name == other.name
            && self.claude == other.claude
    }
    fn off(&self) -> bool {
        self.after == Some(self.claude)
    }
    fn current(&self) -> Result<Option<bool>, String> {
        if self.claude {
            return Ok(Some(
                claude_config::project_settings(&self.file, self.folder.as_deref().unwrap())?
                    .off
                    .contains(&self.name),
            ));
        }
        let text = match std::fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("Couldn't read {}: {e}", self.file.display())),
        };
        let table: toml::Table = toml::from_str(&text)
            .map_err(|e| format!("Couldn't read {}: {e}", self.file.display()))?;
        Ok(table
            .get("mcp_servers")
            .and_then(|t| t.get(&self.name))
            .and_then(|t| t.get("enabled"))
            .and_then(toml::Value::as_bool))
    }
    fn step(&self, value: Option<bool>) -> Step {
        if self.claude {
            Step::ClaudeFlags {
                files: vec![self.file.clone()],
                flags: vec![OffFlag {
                    folder: self.folder.clone().unwrap(),
                    name: self.name.clone(),
                    off: value == Some(true),
                }],
            }
        } else {
            let edit = TomlEdit {
                key: vec!["mcp_servers".into(), self.name.clone(), "enabled".into()],
                value: value.map(toml::Value::Boolean),
            };
            match &self.folder {
                Some(_) => Step::ProjectFile {
                    file: self.file.clone(),
                    edit,
                    shared: false,
                },
                None => Step::UserConfig(edit),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pending {
    key: String,
    choice: Option<Choice>,
    flags: Vec<Flag>,
    id: Option<String>,
    before: Vec<(Agent, Placed)>,
    wants: Vec<(Agent, Placed)>,
    #[serde(default)]
    copies_done: bool,
    #[serde(default)]
    observed: Vec<(Agent, Placed)>,
    #[serde(default)]
    inflight: Option<Agent>,
    #[serde(default)]
    keep_safety: bool,
}

fn execute(flags: &[Flag], reverse: bool, run: Runner<'_>) -> Result<(), String> {
    let mut claude: BTreeMap<PathBuf, Vec<OffFlag>> = BTreeMap::new();
    for flag in flags {
        if !reverse && flag.current()? != flag.before && flag.current()? != flag.after {
            return Err("The settings changed while saving; your edit was kept.".into());
        }
        let value = if reverse { flag.before } else { flag.after };
        if flag.claude {
            claude.entry(flag.file.clone()).or_default().push(OffFlag {
                folder: flag.folder.clone().unwrap(),
                name: flag.name.clone(),
                off: value == Some(true),
            });
        } else {
            run(adapter(Agent::Codex), &[flag.step(value)])?;
        }
    }
    // One locked write and one settle per config file, regardless of project count.
    if !claude.is_empty() {
        let steps: Vec<Step> = claude
            .into_iter()
            .map(|(file, flags)| Step::ClaudeFlags {
                files: vec![file],
                flags,
            })
            .collect();
        run(adapter(Agent::Claude), &steps)?;
    }
    Ok(())
}

pub(super) fn recover(ctx: &Ctx<'_>, sharing: &mut Sharing, run: Runner<'_>) -> Result<(), String> {
    let Some(pending) = sharing.pending.clone() else {
        return Ok(());
    };
    if pending.choice.is_none() {
        let mut finishing = Vec::new();
        for flag in &pending.flags {
            if flag.current()? == flag.before {
                finishing.push(flag.clone());
            }
        }
        execute(&finishing, false, run)?;
        sharing.owned.retain(|f| f.owner != pending.key);
        sharing.chosen.remove(&pending.key);
        sharing.pending = None;
        return sharing.save(ctx.library);
    }
    // Re-establish exclusions before restoring a copy that used to run globally.
    let mut restoring = Vec::new();
    for flag in &pending.flags {
        if flag.current()? == flag.after {
            restoring.push(flag.clone());
        }
    }
    let (off, on): (Vec<_>, Vec<_>) = restoring
        .into_iter()
        .partition(|f| f.before == Some(f.claude));
    execute(&off, true, run)?;
    let mut keep_safety = pending.keep_safety;
    if pending.copies_done
        && let Some(id) = &pending.id
    {
        let mut before = Vec::new();
        for (agent, was) in &pending.before {
            let now = ops::placed(ctx, adapter(*agent), id);
            let seen = pending
                .observed
                .iter()
                .find(|(a, _)| a == agent)
                .map(|(_, p)| p);
            if pending.inflight == Some(*agent)
                || seen.is_some_and(|p| now.same(p))
                || now.same(was)
            {
                before.push((*agent, was.clone()));
            } else {
                keep_safety = true;
            }
        }
        ops::change(ctx, id, &before, run)
            .map_err(|e| format!("Recovery couldn't restore the copies: {e}"))?;
    }
    let (safety, on): (Vec<_>, Vec<_>) = on.into_iter().partition(|f| keep_safety && f.copy);
    for flag in safety {
        sharing.owned.retain(|f| !f.same(&flag));
        sharing.owned.push(flag);
    }
    execute(&on, true, run)?;
    sharing.pending = None;
    sharing.save(ctx.library)?;
    if keep_safety {
        Err(
            "A copy was changed outside Brigadier; your edit was kept, with its safety off flags."
                .into(),
        )
    } else {
        Ok(())
    }
}

pub(super) fn check_restore(ctx: &Ctx<'_>, sharing: &Sharing, key: &str) -> Result<(), String> {
    let catalog = Catalog::read(ctx.roots, None);
    for flag in sharing.owned.iter().filter(|f| f.owner == key) {
        if flag.current()? == flag.after {
            check_steps(
                adapter(if flag.claude {
                    Agent::Claude
                } else {
                    Agent::Codex
                }),
                ctx.roots,
                &catalog,
                &[flag.step(flag.before)],
                ctx.env,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(super) fn restore(
    ctx: &Ctx<'_>,
    sharing: &mut Sharing,
    key: &str,
    run: Runner<'_>,
) -> Result<(), String> {
    check_restore(ctx, sharing, key)?;
    let mut flags = Vec::new();
    for owned in sharing.owned.iter().filter(|f| f.owner == key) {
        if owned.current()? == owned.after {
            let mut restore = owned.clone();
            restore.before = owned.after;
            restore.after = owned.before;
            flags.push(restore);
        }
    }
    sharing.pending = Some(Pending {
        key: key.into(),
        choice: None,
        flags: flags.clone(),
        id: None,
        before: vec![],
        wants: vec![],
        copies_done: false,
        observed: vec![],
        inflight: None,
        keep_safety: false,
    });
    sharing.save(ctx.library)?;
    execute(&flags, false, run)?;
    sharing.owned.retain(|f| f.owner != key);
    sharing.chosen.remove(key);
    sharing.off_scopes.remove(key);
    sharing.failed.remove(key);
    sharing.pending = None;
    sharing.save(ctx.library)
}

fn originals(row: &Row, catalogs: &Catalogs) -> Vec<(Agent, Server)> {
    let mut sources: Vec<(Agent, Server)> = row
        .own
        .sources
        .iter()
        .filter_map(|s| match s {
            Origin::Server { agent, server } => Some((*agent, *server.clone())),
            _ => None,
        })
        .collect();
    // An equivalent under another native name needs the same protection as an original.
    if let Some(endpoint) = row.endpoint() {
        for catalog in
            std::iter::once(&catalogs.user).chain(catalogs.projects.iter().map(|p| &p.catalog))
        {
            for server in catalog.servers.iter().chain(&catalog.off.servers) {
                if server.plugin.is_none()
                    && !server.overrides_user
                    && model::endpoint(server) == endpoint
                    && let Some(agent) = brigadier_registry::adapter::owner(server.source)
                    && !sources
                        .iter()
                        .any(|(a, s)| *a == agent && s.id == server.id)
                {
                    sources.push((agent, server.clone()));
                }
            }
        }
    }
    sources
}
fn covers(server: &Server, path: &Path, agent: Agent) -> bool {
    if agent == Agent::Claude && server.disabled {
        return false;
    }
    server.project.as_ref().is_none_or(|at| {
        if agent == Agent::Claude {
            claude_config::project_key(at) == claude_config::project_key(path)
        } else {
            at == path
        }
    })
}

/// Reconcile an explicit or remembered choice; no agent file changes until all checks pass.
pub(super) fn action(
    ctx: &Ctx<'_>,
    catalogs: &Catalogs,
    sharing: &mut Sharing,
    row: &Row,
    choice: Choice,
    explicit: bool,
    run: Runner<'_>,
) -> Result<(), String> {
    let before = ctx
        .library
        .entries()
        .into_iter()
        .map(|e| e.name)
        .collect::<Vec<_>>();
    let result = action_inner(ctx, catalogs, sharing, row, choice, explicit, run);
    if result.is_err() && sharing.pending.is_none() {
        let mut cleaned = false;
        for entry in ctx
            .library
            .entries()
            .into_iter()
            .filter(|e| !before.contains(&e.name))
        {
            if !adapters().into_iter().any(|a| {
                a.installed(ctx.roots, &entry.id())
                    || !a.installed_in_projects(ctx.roots, &entry.id()).is_empty()
            }) {
                ctx.library.remove(&entry.name).map_err(|e| e.to_string())?;
                sharing.items.remove(&entry.name);
                cleaned = true;
            }
        }
        if cleaned {
            sharing.save(ctx.library)?;
        }
    }
    result
}

fn action_inner(
    ctx: &Ctx<'_>,
    catalogs: &Catalogs,
    sharing: &mut Sharing,
    row: &Row,
    choice: Choice,
    explicit: bool,
    run: Runner<'_>,
) -> Result<(), String> {
    let key = row.key.text();
    let managed = explicit || sharing.chosen.contains_key(&key);
    let sources = originals(row, catalogs);
    for file in std::iter::once(&ctx.roots.claude_json).chain(ctx.claude_configs) {
        claude_config::project_settings(file, &ctx.roots.home)?;
    }
    for source in &row.own.sources {
        if catalogs.unreadable(&source.file()) {
            return Err(format!(
                "Couldn't read {}: try again after the file is readable.",
                source.file().display()
            ));
        }
    }
    for (at, path) in ctx.projects.iter().enumerate() {
        for other in &ctx.projects[..at] {
            if claude_config::project_key(path) == claude_config::project_key(other)
                && choice.at(path) != choice.at(other)
            {
                return Err(format!(
                    "{} and {} share one Claude Code setting (one repository): pick both or neither.",
                    ops::folder_name(path),
                    ops::folder_name(other)
                ));
            }
        }
    }
    for (agent, server) in &sources {
        if *agent == Agent::Claude && server.source == Source::ClaudeProject {
            for project in &catalogs.projects {
                if covers(server, &project.path, *agent)
                    && choice.at(&project.path)
                    && project.catalog.claude_approval(&server.name) != Some(Approval::Approved)
                {
                    return Err(format!(
                        "Claude Code hasn't approved {} in {}: approve it in Claude Code first.",
                        server.name, project.name
                    ));
                }
            }
        }
    }
    let mut flags = Vec::new();
    let mut desired = Vec::new();
    let mut add = |mut flag: Flag| -> Result<(), String> {
        flag.before = flag.current()?;
        if !desired.iter().any(|known: &Flag| known.same(&flag)) {
            desired.push(flag.clone());
        }
        if flag.before != flag.after && !flags.iter().any(|known: &Flag| known.same(&flag)) {
            flags.push(flag);
        }
        Ok(())
    };
    let files: Vec<PathBuf> = std::iter::once(ctx.roots.claude_json.clone())
        .chain(ctx.claude_configs.iter().cloned())
        .collect();
    for (agent, server) in &sources {
        if !managed {
            continue;
        }
        match agent {
            Agent::Claude => {
                let mut places = ctx.projects.to_vec();
                if let Some(path) = &server.project
                    && !places
                        .iter()
                        .any(|p| claude_config::project_key(p) == claude_config::project_key(path))
                    && (!choice.on
                        || sharing.owned.iter().any(|f| {
                            f.owner == key
                                && !f.copy
                                && f.folder == Some(claude_config::project_key(path))
                                && f.name == server.name
                        }))
                {
                    places.push(path.clone());
                }
                for path in places.into_iter().filter(|p| covers(server, p, *agent)) {
                    for file in &files {
                        add(Flag {
                            owner: key.clone(),
                            file: file.clone(),
                            folder: Some(claude_config::project_key(&path)),
                            name: server.name.clone(),
                            claude: true,
                            copy: false,
                            before: None,
                            after: Some(if ctx.projects.contains(&path) {
                                !choice.at(&path)
                            } else {
                                !choice.on
                            }),
                        })?;
                    }
                }
            }
            Agent::Codex => {
                if server.project.is_none() {
                    for folder in std::iter::once(None).chain(
                        catalogs
                            .projects
                            .iter()
                            .filter(|p| adapter(Agent::Codex).trusts(&p.catalog, &p.path))
                            .map(|p| Some(p.path.clone())),
                    ) {
                        let file = folder
                            .as_ref()
                            .map(|p| brigadier_registry::adapter::codex::project_file(p))
                            .unwrap_or_else(|| ctx.roots.codex_home.join("config.toml"));
                        let mut flag = Flag {
                            owner: key.clone(),
                            file,
                            folder: folder.clone(),
                            name: server.name.clone(),
                            claude: false,
                            copy: false,
                            before: None,
                            after: None,
                        };
                        let now = flag.current()?;
                        let all = choice.on && choice.scope.is_none();
                        flag.after = match folder {
                            None => {
                                if all && now.is_none() {
                                    None
                                } else {
                                    Some(all)
                                }
                            }
                            Some(ref path)
                                if choice.on && choice.scope.is_some() && choice.at(path) =>
                            {
                                Some(true)
                            }
                            Some(_) if all && now == Some(false) => Some(true),
                            Some(_) if !all && now == Some(true) => Some(false),
                            Some(_) => now,
                        };
                        add(flag)?;
                    }
                } else if let Some(path) = &server.project {
                    let project = catalogs
                        .projects
                        .iter()
                        .find(|p| p.path == *path)
                        .ok_or_else(|| {
                            "This Codex project isn't a Brigadier project.".to_owned()
                        })?;
                    if !adapter(*agent).trusts(&project.catalog, path) {
                        return Err(
                            "Codex only reads this project's settings once you trust the project."
                                .into(),
                        );
                    }
                    add(Flag {
                        owner: key.clone(),
                        file: server.file.clone(),
                        folder: Some(path.clone()),
                        name: server.name.clone(),
                        claude: false,
                        copy: false,
                        before: None,
                        after: Some(choice.at(path)),
                    })?;
                }
            }
        }
    }
    let mut wants = Vec::new();
    let mut lib = row.lib.clone();
    for adapter in adapters() {
        let agent = adapter.agent();
        let mine: Vec<&Server> = sources
            .iter()
            .filter(|(a, _)| *a == agent)
            .map(|(_, s)| s)
            .collect();
        let global_original = mine
            .iter()
            .any(|s| s.project.is_none() && covers(s, &ctx.roots.home, agent));
        let remaining: Vec<PathBuf> = ctx
            .projects
            .iter()
            .filter(|path| choice.at(path) && !mine.iter().any(|s| covers(s, path, agent)))
            .cloned()
            .collect();
        let global = choice.on && choice.scope.is_none() && !global_original;
        let needed = global || !remaining.is_empty();
        let installed = lib.as_ref().is_some_and(|lib| {
            adapter.installed(ctx.roots, &plugin_id(lib))
                || !adapter
                    .installed_in_projects(ctx.roots, &plugin_id(lib))
                    .is_empty()
        });
        let keep_off = !choice.on && (installed || (mine.is_empty() && !managed));
        if (needed || keep_off) && lib.is_none() {
            if row.conflict {
                return Err("Two different setups have this name: choose one setup first.".into());
            }
            if sharing.is_declined(row) {
                return Err("Share with every agent first: you removed its Brigadier copy.".into());
            }
            if let Some(reason) = cant_share(row) {
                return Err(reason);
            }
            if lib.is_none() {
                let original = row
                    .original()
                    .ok_or_else(|| "The original is no longer available to copy.".to_owned())?;
                let name = library_name(ctx, row, catalogs);
                let packed = pack(ctx, &name, original, &row.key.name, false)?;
                sharing.items.insert(
                    name.clone(),
                    Record {
                        kind: row.kind,
                        name: row.key.name.clone(),
                        fingerprint: fingerprint(original),
                        file: original.file(),
                        version: packed.entry.version,
                        applied: BTreeMap::new(),
                        absent_passes: 0,
                    },
                );
                lib = Some(name);
            }
        }
        let mut want = if needed {
            Placed {
                has: true,
                global,
                on_in: remaining.clone(),
            }
        } else if keep_off {
            Placed::off()
        } else {
            Placed::none()
        };
        if let Some(lib) = &lib {
            if row.flag_only(agent) && keep_off {
                let now = ops::placed(ctx, adapter, &plugin_id(lib));
                want.on_in = if now.global {
                    ctx.projects
                        .iter()
                        .filter(|p| adapter.switched_on_in(p, &plugin_id(lib)))
                        .cloned()
                        .collect()
                } else {
                    now.on_in
                };
            }
            if row.flag_only(agent) && needed {
                // A server-only copy is enabled once; its per-project switch is a native flag.
                want = Placed {
                    has: true,
                    global,
                    // Known projects get an install once, even when their server flag is off.
                    // Some remains off in folders Brigadier has never seen.
                    on_in: ctx.projects.to_vec(),
                };
            }
            if agent == Agent::Claude
                && (want.has
                    || adapter.installed(ctx.roots, &plugin_id(lib))
                    || !adapter
                        .installed_in_projects(ctx.roots, &plugin_id(lib))
                        .is_empty())
            {
                let mut places = ctx.projects.to_vec();
                places.extend(mine.iter().filter_map(|s| s.project.clone()));
                places.sort();
                places.dedup();
                for path in places {
                    let original = mine.iter().any(|s| covers(s, &path, agent));
                    let off = !needed || original || !choice.at(&path);
                    for name in row.copy_servers(ctx.library, lib)? {
                        for file in &files {
                            add(Flag {
                                owner: key.clone(),
                                file: file.clone(),
                                folder: Some(claude_config::project_key(&path)),
                                name: name.clone(),
                                claude: true,
                                copy: true,
                                before: None,
                                after: Some(off),
                            })?;
                        }
                    }
                }
            }
            wants.push((agent, want));
        }
    }
    // Release flags for an original/copy that moved away, restoring only our own value.
    let mut released = Vec::new();
    for owned in sharing
        .owned
        .iter()
        .filter(|f| f.owner == key && !desired.iter().any(|d| d.same(f)))
    {
        released.push(owned.clone());
        if owned.current()? == owned.after {
            flags.push(Flag {
                before: owned.after,
                after: owned.before,
                ..owned.clone()
            });
        }
    }
    // Every involved native file must parse even when no write is necessary.
    for flag in &sharing.owned {
        flag.current()?;
    }
    for flag in &flags {
        check_steps(
            adapter(if flag.claude {
                Agent::Claude
            } else {
                Agent::Codex
            }),
            ctx.roots,
            &catalogs.user,
            &[flag.step(flag.after)],
            ctx.env,
        )
        .map_err(|e| e.to_string())?;
    }
    let mut refresh = None;
    if let Some(lib) = &lib
        && let Some(original) = row.original()
        && let Some(record) = sharing.items.get(lib)
    {
        let print = fingerprint(original);
        if record.fingerprint != print {
            let packed = pack(ctx, lib, original, &row.key.name, true)?;
            refresh = Some((
                lib.clone(),
                print,
                original.file(),
                packed.entry.version,
                packed.changed,
            ));
        }
    }
    let id = lib.as_deref().map(plugin_id);
    if let Some(id) = &id {
        ops::preflight(ctx, id, &wants)?;
    }
    let before: Vec<(Agent, Placed)> = id
        .as_ref()
        .map(|id| {
            adapters()
                .into_iter()
                .map(|a| (a.agent(), ops::placed(ctx, a, id)))
                .collect()
        })
        .unwrap_or_default();
    let pending = Pending {
        key: key.clone(),
        choice: Some(choice.clone()),
        flags: flags.clone(),
        id,
        observed: before.clone(),
        before,
        wants,
        copies_done: false,
        inflight: None,
        keep_safety: false,
    };
    sharing.pending = Some(pending.clone());
    sharing.save(ctx.library)?;
    let result = (|| -> Result<(), String> {
        execute(
            &flags
                .iter()
                .filter(|f| f.off())
                .cloned()
                .collect::<Vec<_>>(),
            false,
            run,
        )?;
        if let Some((lib, _, _, _, true)) = &refresh {
            for adapter in adapters() {
                if adapter.installed(ctx.roots, &plugin_id(lib)) {
                    ops::refresh_copy(ctx, adapter, &plugin_id(lib), run)?;
                }
            }
        }
        if let Some(lib) = &lib {
            tune(ctx, row, lib, run)?;
        }
        if let Some(id) = &pending.id {
            // Journal every native command, including rollback, so a crash midway through a
            // multi-agent move can resume without treating a later editor change as our own.
            let journal = std::cell::RefCell::new(sharing.clone());
            let tracked = |a: &dyn brigadier_registry::adapter::AgentAdapter, steps: &[Step]| {
                for step in steps {
                    {
                        let mut state = journal.borrow_mut();
                        let pending = state.pending.as_mut().unwrap();
                        pending.copies_done = true;
                        pending.inflight = Some(a.agent());
                        state.save(ctx.library)?;
                    }
                    let result = run(a, std::slice::from_ref(step));
                    {
                        let mut state = journal.borrow_mut();
                        let pending = state.pending.as_mut().unwrap();
                        pending.observed = adapters()
                            .into_iter()
                            .map(|a| (a.agent(), ops::placed(ctx, a, id)))
                            .collect();
                        pending.inflight = None;
                        state.save(ctx.library)?;
                    }
                    result?;
                }
                Ok(())
            };
            let changed = ops::change(ctx, id, &pending.wants, &tracked);
            *sharing = journal.into_inner();
            if let Err(error) = changed {
                if !error.restored
                    && let Some(pending) = sharing.pending.as_mut()
                {
                    pending.keep_safety = error.rollback.len() < pending.wants.len();
                    pending.before = error.rollback;
                }
                return Err(format!(
                    "{}: {}{}",
                    adapter(error.agent).label(),
                    error.reason,
                    error.left.map(|s| format!(" {s}")).unwrap_or_default()
                ));
            }
        }
        if let Some(pending) = &mut sharing.pending {
            pending.copies_done = true;
        }
        sharing.save(ctx.library)?;
        execute(
            &flags
                .iter()
                .filter(|f| !f.off())
                .cloned()
                .collect::<Vec<_>>(),
            false,
            run,
        )?;
        for flag in &flags {
            if flag.current()? != flag.after {
                return Err("The agent changed its settings at the same moment; try again.".into());
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        return match recover(ctx, sharing, run) {
            Ok(()) => Err(format!("The action was put back: {error}")),
            Err(restore) if sharing.pending.is_some() => {
                Err(format!("{error} Recovery is still pending: {restore}"))
            }
            Err(restore) => Err(format!("{error} {restore}")),
        };
    }
    if let Some((lib, fingerprint, file, version, _)) = refresh
        && let Some(record) = sharing.items.get_mut(&lib)
    {
        record.fingerprint = fingerprint;
        record.file = file;
        record.version = version;
    }
    if let Some(lib) = &lib
        && let Some(record) = sharing.items.get_mut(lib)
    {
        record.absent_passes = 0;
        for (agent, want) in &pending.wants {
            if want.has {
                record
                    .applied
                    .insert(agent_key(*agent), record.version.clone());
            } else {
                record.applied.remove(&agent_key(*agent));
            }
        }
    }
    for flag in flags {
        let old = sharing.owned.iter().find(|f| f.same(&flag)).cloned();
        sharing.owned.retain(|f| !f.same(&flag));
        let inherited_on = !flag.claude
            && flag.folder.is_some()
            && sources
                .iter()
                .any(|(a, s)| *a == Agent::Codex && s.name == flag.name && s.project.is_none())
            && !sources.iter().any(|(a, s)| {
                *a == Agent::Codex && s.name == flag.name && s.project == flag.folder
            });
        if !released.iter().any(|f| f.same(&flag)) && (flag.off() || inherited_on) {
            sharing.owned.push(Flag {
                before: old.map(|f| f.before).unwrap_or(flag.before),
                ..flag
            });
        }
    }
    if explicit || sharing.chosen.contains_key(&key) {
        sharing.chosen.insert(key, choice);
    }
    sharing.pending = None;
    sharing.save(ctx.library)
}

/// The user's native edits become the remembered Some choice. Safety exclusions are repaired.
pub(super) fn adopt(ctx: &Ctx<'_>, sharing: &mut Sharing, row: &Row) -> Result<(), String> {
    let key = row.key.text();
    let Some(mut choice) = sharing.chosen.get(&key).cloned() else {
        return Ok(());
    };
    let mut changed = false;
    for owned in sharing.owned.iter().filter(|f| f.owner == key) {
        if owned.copy && owned.folder.as_ref().is_some_and(|p| row.own.sources.iter().any(|s| matches!(s, Origin::Server { agent: Agent::Claude, server } if covers(server, p, Agent::Claude)))) { continue; }
        if !owned.claude && owned.folder.is_none() && owned.current()? != owned.after {
            // A deliberate edit of the user's global Codex switch is a global choice.
            choice.on = owned.current()? != Some(false);
            choice.scope = None;
            changed = true;
            continue;
        }
        if owned.current()? != owned.after
            && let Some(path) = &owned.folder
        {
            let mut places = ctx
                .projects
                .iter()
                .filter(|p| {
                    if owned.claude {
                        claude_config::project_key(p) == *path
                    } else {
                        *p == path
                    }
                })
                .collect::<Vec<_>>();
            if places.is_empty() && owned.claude && !owned.copy
                && row.own.sources.iter().any(|s| matches!(s, Origin::Server { agent: Agent::Claude, server } if server.project.as_ref().is_some_and(|p| claude_config::project_key(p) == *path))) {
                places.push(path);
            }
            for project in places {
                let paths = choice.scope.get_or_insert_with(|| {
                    if choice.on {
                        ctx.projects.to_vec()
                    } else {
                        vec![]
                    }
                });
                let enabled = if owned.claude {
                    owned.current()? != Some(true)
                } else {
                    owned.current()? != Some(false)
                };
                if enabled {
                    if !paths.contains(project) {
                        paths.push(project.clone());
                    }
                    choice.on = true;
                } else {
                    paths.retain(|p| p != project);
                }
                changed = true;
            }
        }
    }
    // An unowned off flag in an on project is the user's own /mcp choice.
    for path in ctx.projects {
        if !choice.at(path) {
            continue;
        }
        let mut probes = Vec::new();
        for source in &row.own.sources {
            if let Origin::Server { agent, server } = source
                && covers(server, path, *agent)
            {
                let claude = *agent == Agent::Claude;
                if claude {
                    for file in std::iter::once(&ctx.roots.claude_json).chain(ctx.claude_configs) {
                        probes.push(Flag {
                            owner: key.clone(),
                            file: file.clone(),
                            folder: Some(claude_config::project_key(path)),
                            name: server.name.clone(),
                            claude,
                            copy: false,
                            before: None,
                            after: Some(true),
                        });
                    }
                } else {
                    probes.push(Flag {
                        owner: key.clone(),
                        file: brigadier_registry::adapter::codex::project_file(path),
                        folder: Some(path.clone()),
                        name: server.name.clone(),
                        claude,
                        copy: false,
                        before: None,
                        after: Some(false),
                    });
                    probes.push(Flag {
                        owner: key.clone(),
                        file: ctx.roots.codex_home.join("config.toml"),
                        folder: None,
                        name: server.name.clone(),
                        claude,
                        copy: false,
                        before: None,
                        after: Some(false),
                    });
                }
            }
        }
        if let Some(lib) = &row.lib && !row.own.sources.iter().any(|s| matches!(s, Origin::Server {agent: Agent::Claude, server} if covers(server, path, Agent::Claude))) {
            for name in row.copy_servers(ctx.library, lib)? {
                for file in std::iter::once(&ctx.roots.claude_json).chain(ctx.claude_configs) {
                    probes.push(Flag { owner: key.clone(), file: file.clone(), folder: Some(claude_config::project_key(path)), name: name.clone(), claude: true, copy: true, before: None, after: Some(true) });
                }
            }
        }
        for probe in probes {
            if probe.current()? == probe.after && !sharing.owned.iter().any(|f| f.same(&probe)) {
                let paths = choice.scope.get_or_insert_with(|| ctx.projects.to_vec());
                paths.retain(|p| p != path);
                changed = true;
            }
        }
    }

    if changed {
        if choice.scope.as_ref().is_some_and(Vec::is_empty) {
            choice.on = false;
        }
        sharing
            .owned
            .retain(|f| f.owner != key || f.current().ok() == Some(f.after));
        sharing.chosen.insert(key, choice);
        sharing.save(ctx.library)?;
    }
    Ok(())
}
