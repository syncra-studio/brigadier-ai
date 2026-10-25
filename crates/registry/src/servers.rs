//! Reading MCP servers from each agent's config.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::{
    Agent, AsWritten, OAuthClient, Plugin, Problem, Roots, Secret, Server, Source, Transport,
    plugins, server_id,
};

/// Whether `text` names a variable: `${…}`.
fn names_variable(text: &str) -> bool {
    text.contains("${")
}

/// How a source writes variables in its values.
#[derive(Clone, Copy)]
enum Variables {
    /// None (Codex's TOML, Claude Desktop).
    None,
    /// Claude Code: `${VAR}` and `${VAR:-default}`.
    Claude,
    /// Cursor: `${env:VAR}`, `${userHome}`, `${workspaceFolder}`, `${workspaceFolderBasename}`,
    /// `${pathSeparator}` and `${/}`.
    Cursor,
}

/// What a config file's servers share.
struct Origin<'a> {
    source: Source,
    project: Option<&'a Path>,
    /// Where relative paths resolve from.
    base: Option<&'a Path>,
    variables: Variables,
    /// The config file.
    file: &'a Path,
    /// A Claude plugin's folder, which `${CLAUDE_PLUGIN_ROOT}` names.
    plugin_root: Option<&'a Path>,
    /// `mcpServers` with its keys in written order.
    written: Option<&'a Written>,
}

/// The user's servers: Claude Code's user and local scopes, Codex's, Claude Desktop's and
/// Cursor's.
pub(crate) fn user(
    roots: &Roots,
    codex: Option<&toml::Table>,
    problems: &mut Vec<Problem>,
) -> Vec<Server> {
    let mut servers = Vec::new();
    if let Some((claude, written)) =
        read_json(&roots.claude_json, Source::ClaudeUser, None, problems)
    {
        if let Some(own) = claude.get("mcpServers").and_then(Value::as_object) {
            servers.extend(json_servers(
                own,
                &Origin {
                    source: Source::ClaudeUser,
                    project: None,
                    base: None,
                    variables: Variables::Claude,
                    file: &roots.claude_json,
                    plugin_root: None,
                    written: written.get("mcpServers"),
                },
                roots,
                problems,
            ));
        }
        for (path, project) in claude
            .get("projects")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let Some(own) = project.get("mcpServers").and_then(Value::as_object) else {
                continue;
            };
            let path = Path::new(path);
            servers.extend(json_servers(
                own,
                &Origin {
                    source: Source::ClaudeLocal,
                    project: Some(path),
                    base: Some(path),
                    variables: Variables::Claude,
                    file: &roots.claude_json,
                    plugin_root: None,
                    written: written
                        .get("projects")
                        .and_then(|projects| projects.get(&path.to_string_lossy()))
                        .and_then(|project| project.get("mcpServers")),
                },
                roots,
                problems,
            ));
        }
    }
    if let Some(codex) = codex {
        let file = roots.codex_home.join("config.toml");
        servers.extend(toml_servers(
            codex,
            Source::CodexUser,
            None,
            &file,
            roots,
            problems,
        ));
    }
    if let Some(file) = roots.desktop_config.as_deref()
        && let Some((desktop, written)) = read_json(file, Source::Desktop, None, problems)
        && let Some(own) = desktop.get("mcpServers").and_then(Value::as_object)
    {
        servers.extend(json_servers(
            own,
            &Origin {
                source: Source::Desktop,
                project: None,
                base: None,
                variables: Variables::None,
                file,
                plugin_root: None,
                written: written.get("mcpServers"),
            },
            roots,
            problems,
        ));
    }
    let file = roots.cursor_dir.join("mcp.json");
    if let Some((cursor, written)) = read_json(&file, Source::CursorUser, None, problems)
        && let Some(own) = cursor.get("mcpServers").and_then(Value::as_object)
    {
        servers.extend(json_servers(
            own,
            &Origin {
                source: Source::CursorUser,
                project: None,
                base: None,
                variables: Variables::Cursor,
                file: &file,
                plugin_root: None,
                written: written.get("mcpServers"),
            },
            roots,
            problems,
        ));
    }
    servers
}

/// A project's servers: its `.mcp.json`, its Codex layers and its Cursor config.
pub(crate) fn project(roots: &Roots, project: &Path, problems: &mut Vec<Problem>) -> Vec<Server> {
    let mut servers = Vec::new();
    let file = project.join(".mcp.json");
    if let Some((config, written)) =
        read_json(&file, Source::ClaudeProject, Some(project), problems)
        && let Some(own) = config.get("mcpServers").and_then(Value::as_object)
    {
        servers.extend(json_servers(
            own,
            &Origin {
                source: Source::ClaudeProject,
                project: Some(project),
                base: Some(project),
                variables: Variables::Claude,
                file: &file,
                plugin_root: None,
                written: written.get("mcpServers"),
            },
            roots,
            problems,
        ));
    }
    servers.extend(codex_layers(roots, project, project, problems));
    let file = project.join(".cursor").join("mcp.json");
    if let Some((config, written)) =
        read_json(&file, Source::CursorProject, Some(project), problems)
        && let Some(own) = config.get("mcpServers").and_then(Value::as_object)
    {
        servers.extend(json_servers(
            own,
            &Origin {
                source: Source::CursorProject,
                project: Some(project),
                base: Some(project),
                variables: Variables::Cursor,
                file: &file,
                plugin_root: None,
                written: written.get("mcpServers"),
            },
            roots,
            problems,
        ));
    }
    servers
}

/// The servers of the project's `.codex/config.toml` layers that a session working in `dir`
/// loads: every folder from `dir` up to the project's root, the nearest winning for a name, as
/// Codex merges them. Each starts in its layer's folder unless it names its own.
pub(crate) fn codex_layers(
    roots: &Roots,
    project: &Path,
    dir: &Path,
    problems: &mut Vec<Problem>,
) -> Vec<Server> {
    let user = codex_config(roots, &mut Vec::new());
    let mut servers: Vec<Server> = Vec::new();
    for folder in dir.ancestors() {
        let file = folder.join(".codex").join("config.toml");
        if let Some(mut layer) = read_toml(&file, Source::CodexProject, Some(project), problems) {
            // An entry that names no command or url of its own only switches the user's server
            // of its name: it is that server, switched here, not a server of the project's.
            let switches: Vec<String> = layer
                .get("mcp_servers")
                .and_then(toml::Value::as_table)
                .into_iter()
                .flatten()
                .filter(|(_, entry)| {
                    entry.as_table().is_some_and(|table| {
                        table.contains_key("enabled") && table.keys().all(|key| key == "enabled")
                    })
                })
                .map(|(name, _)| name.clone())
                .collect();
            inherit(&mut layer, user.as_ref());
            for mut server in toml_servers_with(
                &layer,
                Source::CodexProject,
                Some((project, folder)),
                &file,
                roots,
                problems,
                true,
            ) {
                server.overrides_user = switches.contains(&server.name);
                // Switched off over the user's server: what `codex_off_here` tells.
                if server.disabled && server.overrides_user {
                    continue;
                }
                if !servers.iter().any(|known| known.name == server.name) {
                    servers.push(server);
                }
            }
        }
        if folder == project {
            break;
        }
    }
    servers
}

/// A project layer's servers over the user's entries of the same names, as Codex merges its
/// layers: an entry that only switches a server (`enabled`) keeps the user's command or url.
fn inherit(layer: &mut toml::Table, user: Option<&toml::Table>) {
    let Some(user) = user
        .and_then(|user| user.get("mcp_servers"))
        .and_then(toml::Value::as_table)
    else {
        return;
    };
    let Some(own) = layer
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    for (name, entry) in own.iter_mut() {
        if let (Some(toml::Value::Table(under)), toml::Value::Table(over)) =
            (user.get(name), &*entry)
        {
            let mut merged = under.clone();
            merge_toml(&mut merged, over);
            *entry = toml::Value::Table(merged);
        }
    }
}

/// `over` merged into `under`, table by table.
fn merge_toml(under: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (under.get_mut(key), value) {
            (Some(toml::Value::Table(under)), toml::Value::Table(over)) => merge_toml(under, over),
            _ => {
                under.insert(key.clone(), value.clone());
            }
        }
    }
}

/// `$CODEX_HOME/config.toml`, read once for its servers, skills and trust.
pub(crate) fn codex_config(roots: &Roots, problems: &mut Vec<Problem>) -> Option<toml::Table> {
    read_toml(
        &roots.codex_home.join("config.toml"),
        Source::CodexUser,
        None,
        problems,
    )
}

/// The user's own `skills.config` entries.
pub(crate) fn codex_skill_config(config: &toml::Table) -> Vec<Value> {
    config
        .get("skills")
        .and_then(|skills| skills.get("config"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| serde_json::to_value(entry).ok())
        .collect()
}

/// The projects the user trusts in Codex (`[projects."<path>"] trust_level = "trusted"`).
pub(crate) fn codex_trusted(config: &toml::Table) -> Vec<PathBuf> {
    config
        .get("projects")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter(|(_, project)| {
            project.get("trust_level").and_then(toml::Value::as_str) == Some("trusted")
        })
        .map(|(path, _)| PathBuf::from(path))
        .collect()
}

/// `mcpServers` of a JSON config (Claude Code, Claude Desktop, Cursor).
fn json_servers(
    own: &Map<String, Value>,
    origin: &Origin<'_>,
    roots: &Roots,
    problems: &mut Vec<Problem>,
) -> Vec<Server> {
    let expand = |text: &str| expand(text, origin, roots);
    let mut problem = |name: &str, read: Option<String>, error: String| {
        problems.push(entry_problem(
            name,
            origin.source,
            origin.project,
            origin.file,
            read,
            error,
        ));
    };
    let mut servers = Vec::new();
    for (name, config) in own {
        let Some(config) = config.as_object() else {
            continue;
        };
        // A server switched off where it is configured is off for every session, but still
        // the user's: kept as switched off.
        let disabled = config.get("disabled").and_then(Value::as_bool) == Some(true);
        let raw = |key: &str| config.get(key).and_then(Value::as_str).map(str::to_owned);
        let written = AsWritten {
            command: raw("command"),
            args: config
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            url: raw("url"),
            cwd: raw("cwd"),
        };
        let templated = !matches!(origin.variables, Variables::None)
            && (written.command.as_deref().is_some_and(names_variable)
                || written.args.iter().any(|arg| names_variable(arg))
                || written.url.as_deref().is_some_and(names_variable)
                || written.cwd.as_deref().is_some_and(names_variable)
                || config
                    .get("oauth")
                    .and_then(|oauth| oauth.get("clientId"))
                    .and_then(Value::as_str)
                    .is_some_and(names_variable)
                || ["env", "headers"].iter().any(|key| {
                    string_map(config.get(*key)).any(|(_, value)| names_variable(&value))
                }));
        let kind = config.get("type").and_then(Value::as_str);
        let transport = if let Some(url) = config.get("url").and_then(Value::as_str) {
            // Legacy SSE servers aren't shared: Codex has no SSE transport.
            if kind == Some("sse") {
                continue;
            }
            let raw_url = url;
            let url = expand(url);
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                let scheme = raw_url.split_once("://").map_or("", |(scheme, _)| scheme);
                // As written: what a variable expands to never leaves the config.
                problem(
                    name,
                    Some(raw_url.to_owned()),
                    format!("Unsupported address “{scheme}://”: only http and https"),
                );
                continue;
            }
            let mut headers: Vec<(String, Secret)> = string_map(config.get("headers"))
                .map(|(header, value)| (header, Secret::new(expand(&value))))
                .collect();
            // Claude Code's sign-in key hashes them in the order they are written.
            if let Some(written) = origin
                .written
                .and_then(|servers| servers.get(name))
                .and_then(|server| server.get("headers"))
            {
                headers.sort_by_key(|(header, _)| written.position(header));
            }
            Transport::Http { url, headers }
        } else if let Some(command) = config.get("command").and_then(Value::as_str) {
            if let Some(kind) = kind.filter(|kind| *kind != "stdio") {
                problem(
                    name,
                    Some(command.to_owned()),
                    format!("Unknown type “{kind}”"),
                );
                continue;
            }
            let base = origin.base;
            Transport::Stdio {
                command: command_path(&expand(command), base),
                args: config
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(expand)
                    .collect(),
                env: string_map(config.get("env"))
                    .map(|(key, value)| (key, Secret::new(expand(&value))))
                    .collect(),
                cwd: config
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(|cwd| absolute(&expand(cwd), base)),
                base: base.map(Path::to_owned),
            }
        } else {
            problem(name, None, "It has no command or url".into());
            continue;
        };
        let mut server = server(name, origin.source, origin.project, origin.file, transport);
        server.disabled = disabled;
        server.written = written;
        server.templated = templated;
        server.oauth = config
            .get("oauth")
            .and_then(|oauth| oauth.get("clientId"))
            .and_then(Value::as_str)
            .map(|id| OAuthClient {
                client_id: Some(expand(id)),
                client_secret: None,
            });
        servers.push(server);
    }
    servers
}

/// The user's own Codex servers set `enabled = false`, read as they'd be once on.
pub(crate) fn toml_servers_off(config: &toml::Table, roots: &Roots) -> Vec<Server> {
    let off: toml::Table = config
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter_map(|(name, entry)| {
            let mut entry = entry.as_table()?.clone();
            (entry.remove("enabled").and_then(|on| on.as_bool()) == Some(false))
                .then(|| (name.clone(), toml::Value::Table(entry)))
        })
        .collect();
    if off.is_empty() {
        return Vec::new();
    }
    let mut config = toml::Table::new();
    config.insert("mcp_servers".to_owned(), toml::Value::Table(off));
    toml_servers(
        &config,
        Source::CodexUser,
        None,
        &roots.codex_home.join("config.toml"),
        roots,
        &mut Vec::new(),
    )
}

/// `mcp_servers` of a Codex config. `layer`: a project's, with the folder of the layer, which
/// its servers start in by default.
fn toml_servers(
    config: &toml::Table,
    source: Source,
    layer: Option<(&Path, &Path)>,
    file: &Path,
    roots: &Roots,
    problems: &mut Vec<Problem>,
) -> Vec<Server> {
    toml_servers_with(config, source, layer, file, roots, problems, false)
}

/// [`toml_servers`]; `keep_disabled`: an entry set `enabled = false` comes too, marked
/// [`Server::disabled`] (a project's own server switched off is still the user's).
fn toml_servers_with(
    config: &toml::Table,
    source: Source,
    layer: Option<(&Path, &Path)>,
    file: &Path,
    roots: &Roots,
    problems: &mut Vec<Problem>,
    keep_disabled: bool,
) -> Vec<Server> {
    let Some(own) = config.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    let base = layer.map(|(_, folder)| folder);
    let env_var = |name: &str| roots.vars.get(name).cloned().unwrap_or_default();
    let mut servers = Vec::new();
    for (name, config) in own {
        let Some(config) = config.as_table() else {
            continue;
        };
        let disabled = config.get("enabled").and_then(toml::Value::as_bool) == Some(false);
        if disabled && !keep_disabled {
            continue;
        }
        let strings = |key: &str| -> Vec<(String, String)> {
            config
                .get(key)
                .and_then(toml::Value::as_table)
                .into_iter()
                .flatten()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
                .collect()
        };
        let transport = if let Some(url) = config.get("url").and_then(toml::Value::as_str) {
            let mut headers: Vec<(String, Secret)> = strings("http_headers")
                .into_iter()
                .map(|(header, value)| (header, Secret::new(value)))
                .collect();
            headers.extend(
                strings("env_http_headers")
                    .into_iter()
                    .map(|(header, var)| (header, Secret::new(env_var(&var)))),
            );
            if let Some(var) = config
                .get("bearer_token_env_var")
                .and_then(toml::Value::as_str)
            {
                headers.push((
                    "Authorization".into(),
                    Secret::new(format!("Bearer {}", env_var(var))),
                ));
            }
            Transport::Http {
                url: url.to_owned(),
                headers,
            }
        } else if let Some(command) = config.get("command").and_then(toml::Value::as_str) {
            let mut env: Vec<(String, Secret)> = strings("env")
                .into_iter()
                .map(|(key, value)| (key, Secret::new(value)))
                .collect();
            // Variables Codex passes on from its own environment, by name (or `{ name }`).
            for entry in config
                .get("env_vars")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = entry
                    .as_str()
                    .or_else(|| entry.get("name").and_then(toml::Value::as_str));
                if let Some(name) = name
                    && let Some(value) = roots.vars.get(name)
                {
                    env.push((name.to_owned(), Secret::new(value.clone())));
                }
            }
            Transport::Stdio {
                command: command_path(command, base),
                args: config
                    .get("args")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                env,
                cwd: config
                    .get("cwd")
                    .and_then(toml::Value::as_str)
                    .map(|cwd| absolute(cwd, base)),
                base: base.map(Path::to_owned),
            }
        } else {
            problems.push(entry_problem(
                name,
                source,
                layer.map(|(project, _)| project),
                file,
                None,
                "It has no command or url".into(),
            ));
            continue;
        };
        let mut server = server(
            name,
            source,
            layer.map(|(project, _)| project),
            file,
            transport,
        );
        let raw = |key: &str| {
            config
                .get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        };
        server.written = AsWritten {
            command: raw("command"),
            args: config
                .get("args")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect(),
            url: raw("url"),
            cwd: raw("cwd"),
        };
        server.templated = ["env_vars", "env_http_headers", "bearer_token_env_var"]
            .iter()
            .any(|key| config.contains_key(*key))
            || server
                .written
                .command
                .as_deref()
                .is_some_and(names_variable)
            || server.written.args.iter().any(|arg| names_variable(arg))
            || server.written.url.as_deref().is_some_and(names_variable)
            || server.written.cwd.as_deref().is_some_and(names_variable);
        server.disabled = disabled;
        server.oauth = config
            .get("oauth")
            .and_then(toml::Value::as_table)
            .map(|oauth| {
                let text = |key: &str| oauth.get(key).and_then(toml::Value::as_str);
                OAuthClient {
                    client_id: text("client_id").map(str::to_owned),
                    client_secret: text("client_secret").map(Secret::new),
                }
            });
        servers.push(server);
    }
    servers
}

fn server(
    name: &str,
    source: Source,
    project: Option<&Path>,
    file: &Path,
    transport: Transport,
) -> Server {
    let signs_in = matches!(&transport, Transport::Http { headers, .. }
        if !headers.iter().any(|(header, _)| header.eq_ignore_ascii_case("authorization")));
    Server {
        id: server_id(source, project, name),
        name: name.to_owned(),
        source,
        project: project.map(Path::to_owned),
        transport,
        signs_in,
        plugin: None,
        file: file.to_owned(),
        oauth: None,
        disabled: false,
        written: AsWritten::default(),
        templated: false,
        overrides_user: false,
    }
}

/// The MCP servers of `plugin`, as its agent reads them: Claude's with its variables and
/// `${CLAUDE_PLUGIN_ROOT}`, Codex's in its own keys, less those its `config.toml` switches off.
/// A relative command or folder is in the plugin's.
pub(crate) fn plugin(
    roots: &Roots,
    plugin: &Plugin,
    codex: Option<&toml::Table>,
    problems: &mut Vec<Problem>,
) -> Vec<Server> {
    let Some((file, own)) = plugin.server_file() else {
        return Vec::new();
    };
    let (source, start) = match plugin.agent {
        Agent::Claude => (Source::ClaudePlugin, problems.len()),
        Agent::Codex => (Source::CodexPlugin, problems.len()),
    };
    let mut servers = match (own, plugin.agent) {
        (None, _) => {
            problems.push(file_problem(&file, source, None, None));
            Vec::new()
        }
        (Some(Value::Object(own)), Agent::Claude) => json_servers(
            &own,
            &Origin {
                source,
                project: None,
                base: Some(&plugin.root),
                variables: Variables::Claude,
                file: &file,
                plugin_root: Some(&plugin.root),
                written: None,
            },
            roots,
            problems,
        ),
        (Some(own @ Value::Object(_)), Agent::Codex) => {
            match serde_json::from_value::<toml::Table>(serde_json::json!({ "mcp_servers": own })) {
                Ok(config) => {
                    let off = plugins::codex_servers_off(codex, &plugin.id);
                    let mut servers = toml_servers(
                        &config,
                        source,
                        Some((&plugin.root, &plugin.root)),
                        &file,
                        roots,
                        problems,
                    );
                    servers.retain(|server| !off.contains(&server.name));
                    servers
                }
                Err(_) => {
                    problems.push(file_problem(&file, source, None, None));
                    Vec::new()
                }
            }
        }
        (Some(_), _) => Vec::new(),
    };
    servers.retain(|server| !server.disabled);
    for server in &mut servers {
        server.id = format!("{}:{}", plugin.prefix(), server.name);
        server.project.clone_from(&plugin.project);
        server.plugin = Some(plugin.clone());
        server.file.clone_from(&file);
    }
    for problem in &mut problems[start..] {
        problem.id = format!("{}:{}", plugin.prefix(), problem.name);
        problem.project.clone_from(&plugin.project);
    }
    servers
}

/// A JSON object's string values.
fn string_map(value: Option<&Value>) -> impl Iterator<Item = (String, String)> + '_ {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
}

/// A command as its source means it: one written as a relative path (`./bin/server`,
/// `scripts/mcp`) is in `base`; a bare name is looked up on PATH.
fn command_path(command: &str, base: Option<&Path>) -> String {
    let relative = Path::new(command).is_relative()
        && (command.contains('/') || (cfg!(windows) && command.contains('\\')));
    match base {
        Some(base) if relative => base.join(command).display().to_string(),
        _ => command.to_owned(),
    }
}

fn absolute(path: &str, base: Option<&Path>) -> PathBuf {
    match base {
        Some(base) if Path::new(path).is_relative() => base.join(path),
        _ => PathBuf::from(path),
    }
}

/// `text` with the source's variables expanded from the CLIs' environment. An unset variable
/// without a default expands to nothing.
fn expand(text: &str, origin: &Origin<'_>, roots: &Roots) -> String {
    let (variables, project) = (origin.variables, origin.project);
    if matches!(variables, Variables::None) || !text.contains("${") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..end];
        let var = |name: &str| match origin.plugin_root {
            Some(root) if name == "CLAUDE_PLUGIN_ROOT" => Some(root.display().to_string()),
            _ => roots.vars.get(name).cloned(),
        };
        let value = match variables {
            Variables::Claude => match inner.split_once(":-") {
                Some((name, default)) => var(name)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| default.to_owned()),
                None => var(inner).unwrap_or_default(),
            },
            Variables::Cursor => match inner {
                "userHome" => roots.home.display().to_string(),
                "workspaceFolder" => project.map(|p| p.display().to_string()).unwrap_or_default(),
                "workspaceFolderBasename" => project
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "pathSeparator" | "/" => std::path::MAIN_SEPARATOR.to_string(),
                _ => match inner.strip_prefix("env:") {
                    Some(name) => var(name).unwrap_or_default(),
                    None => format!("${{{inner}}}"),
                },
            },
            Variables::None => unreachable!(),
        };
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A server entry of `file` that can't be used.
fn entry_problem(
    name: &str,
    source: Source,
    project: Option<&Path>,
    file: &Path,
    read: Option<String>,
    error: String,
) -> Problem {
    Problem {
        id: server_id(source, project, name),
        name: name.to_owned(),
        source,
        project: project.map(Path::to_owned),
        file: file.to_owned(),
        line: None,
        read,
        error,
    }
}

/// A config file that can't be parsed: none of its servers can be read.
fn file_problem(
    path: &Path,
    source: Source,
    project: Option<&Path>,
    line: Option<usize>,
) -> Problem {
    Problem {
        id: format!("{}:file:{}", source.prefix(), path.display()),
        name: path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        source,
        project: project.map(Path::to_owned),
        file: path.to_owned(),
        line,
        read: None,
        error: match line {
            Some(line) => format!("The file couldn't be parsed (line {line})"),
            None => "The file couldn't be parsed".into(),
        },
    }
}

/// A JSON config, and its objects' keys in written order when it has headers.
fn read_json(
    path: &Path,
    source: Source,
    project: Option<&Path>,
    problems: &mut Vec<Problem>,
) -> Option<(Value, Written)> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => {
            let written = if text.contains("\"headers\"") {
                serde_json::from_str(&text).unwrap_or_default()
            } else {
                Written::default()
            };
            Some((value, written))
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "unreadable MCP config");
            problems.push(file_problem(path, source, project, Some(error.line())));
            None
        }
    }
}

pub(crate) fn read_toml(
    path: &Path,
    source: Source,
    project: Option<&Path>,
    problems: &mut Vec<Problem>,
) -> Option<toml::Table> {
    let text = std::fs::read_to_string(path).ok()?;
    match text.parse::<toml::Table>() {
        Ok(table) => Some(table),
        Err(error) => {
            // The error's own text quotes the line, which may hold a secret.
            tracing::warn!(
                path = %path.display(),
                error = error.message(),
                at = ?error.span(),
                "unreadable Codex config"
            );
            let line = error
                .span()
                .and_then(|span| text.get(..span.start))
                .map(|before| before.matches('\n').count() + 1);
            problems.push(file_problem(path, source, project, line));
            None
        }
    }
}

/// A JSON value's objects with their keys in the order they are written (serde_json's `Map`
/// sorts them). Arrays and scalars keep nothing.
#[derive(Default)]
struct Written(Vec<(String, Written)>);

impl Written {
    fn get(&self, key: &str) -> Option<&Written> {
        self.0
            .iter()
            .find(|(own, _)| own == key)
            .map(|(_, value)| value)
    }

    /// Where `key` is written first, as JavaScript keeps a repeated key.
    fn position(&self, key: &str) -> Option<usize> {
        self.0.iter().position(|(own, _)| own == key)
    }
}

impl<'de> Deserialize<'de> for Written {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(WrittenVisitor)
    }
}

struct WrittenVisitor;

impl<'de> Visitor<'de> for WrittenVisitor {
    type Value = Written;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("JSON")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Written, A::Error> {
        let mut keys = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            keys.push((key, map.next_value()?));
        }
        Ok(Written(keys))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Written, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Written::default())
    }

    fn visit_bool<E>(self, _: bool) -> Result<Written, E> {
        Ok(Written::default())
    }

    fn visit_i64<E>(self, _: i64) -> Result<Written, E> {
        Ok(Written::default())
    }

    fn visit_u64<E>(self, _: u64) -> Result<Written, E> {
        Ok(Written::default())
    }

    fn visit_f64<E>(self, _: f64) -> Result<Written, E> {
        Ok(Written::default())
    }

    fn visit_str<E>(self, _: &str) -> Result<Written, E> {
        Ok(Written::default())
    }

    fn visit_unit<E>(self) -> Result<Written, E> {
        Ok(Written::default())
    }
}
