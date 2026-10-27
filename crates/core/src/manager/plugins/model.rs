//! Provenance-preserving view shared by the page and resolver.
use super::PluginKind;
use brigadier_registry::{Server, Transport};
/// The read-only page model shared with the resolver.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    pub rows: Vec<CatalogRow>,
    pub removed: Vec<RemovedPlugin>,
    /// Hash of catalog + choices for later menu cache invalidation.
    pub version: String,
}

/// Machine-found items hidden from Brigadier; native files remain untouched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct RemovedPlugin {
    pub id: String,
    pub kind: PluginKind,
    pub name: String,
    pub agents: Vec<brigadier_providers::ProviderKind>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRow {
    pub id: String,
    pub kind: PluginKind,
    pub name: String,
    pub definitions: Vec<CatalogDefinition>,
    pub different_per_agent: bool,
    pub choice: super::choices::Choice,
    pub sign_in: CatalogSignIn,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDefinition {
    pub agent: Option<brigadier_providers::ProviderKind>,
    pub file: String,
    pub plugin: Option<String>,
    pub project: Option<String>,
    pub native: String,
    pub aliases: Vec<String>,
    pub fingerprint: String,
    pub transport: Option<String>,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CatalogSignIn {
    #[default]
    NotRequired,
    SignedOut,
    SigningIn {
        flow_id: String,
    },
    SignedIn,
    CliFallback {
        agents: Vec<brigadier_providers::ProviderKind>,
        #[serde(default)]
        steps: Vec<super::super::mcp_login::AgentSignIn>,
    },
    CliSetupRequired {
        agents: Vec<brigadier_providers::ProviderKind>,
    },
    Failed {
        message: String,
    },
}

impl CatalogView {
    pub fn new(
        rows: &[brigadier_registry::catalog::Row],
        choices: &super::choices::Choices,
    ) -> Self {
        use brigadier_registry::catalog::{Content, Kind};
        let rows: Vec<_> = rows.iter().filter(|row| !choices.is_removed(&row.id)).map(|row| CatalogRow {
            id: row.id.clone(), kind: if row.kind == Kind::Server { PluginKind::Server } else { PluginKind::Skill },
            name: row.name.clone(), different_per_agent: row.different_per_agent,
            choice: choices.choice(row),
            sign_in: if row.definitions.iter().any(|definition| matches!(&definition.content, Content::Server(server) if server.signs_in)) { CatalogSignIn::SignedOut } else { CatalogSignIn::NotRequired },
            definitions: row.definitions.iter().map(|definition| {
                let origin = &definition.provenance;
                let (transport, endpoint) = match &definition.content {
                    Content::Server(server) => match &server.transport {
                        Transport::Http { .. } => (Some("http".to_owned()), server.written.url.clone()),
                        Transport::Stdio { .. } => (Some("stdio".to_owned()), server.written.command.clone()),
                    },
                    Content::Skill { .. } => (None, None),
                };
                CatalogDefinition { agent: origin.agent.map(super::provider), file: origin.file.to_string_lossy().into_owned(), plugin: origin.plugin.clone(), project: origin.project.as_ref().map(|path| path.to_string_lossy().into_owned()), native: origin.native.clone(), aliases: origin.aliases.clone(), fingerprint: origin.fingerprint.clone(), transport, endpoint }
            }).collect(),
        }).collect();
        let removed: Vec<_> = choices
            .0
            .values()
            .filter_map(|choice| choice.removed.clone())
            .collect();
        let bytes = serde_json::to_vec(&(&rows, &removed)).expect("catalog rows are serializable");
        Self {
            rows,
            removed,
            version: brigadier_registry::catalog::fingerprint(&bytes),
        }
    }
}

// Legacy identity retained solely for importing declined choices.
pub(crate) fn endpoint(server: &Server) -> String {
    let mut text = reach(server);
    text.push_str(&credentials(server));
    text
}

fn credentials(server: &Server) -> String {
    let mut text = String::new();
    let values = match &server.transport {
        Transport::Stdio { env, .. } => env,
        Transport::Http { headers, .. } => headers,
    };
    let mut values: Vec<String> = values
        .iter()
        .map(|(name, value)| {
            format!(
                "{name}={}",
                brigadier_registry::library::hash_text(value.expose())
            )
        })
        .collect();
    values.sort();
    for value in values {
        text.push('\0');
        text.push_str(&value);
    }
    if let Some(oauth) = &server.oauth {
        text.push_str(&format!(
            "\0oauth {} {}",
            oauth.client_id.as_deref().unwrap_or_default(),
            oauth
                .client_secret
                .as_ref()
                .map(|secret| brigadier_registry::library::hash_text(secret.expose()))
                .unwrap_or_default()
        ));
    }
    text
}

fn reach(server: &Server) -> String {
    let written = &server.written;
    match &server.transport {
        Transport::Http { url, .. } => {
            format!("url {}", written.url.as_deref().unwrap_or(url))
        }
        Transport::Stdio { command, args, .. } => {
            let command = written.command.as_deref().unwrap_or(command);
            let args = if written.command.is_some() {
                &written.args
            } else {
                args
            };
            let folder = server
                .transport
                .working_dir()
                .map(|dir| dir.display().to_string())
                .unwrap_or_default();
            format!("run {command}\0{}\0{folder}", args.join("\0"))
        }
    }
}
