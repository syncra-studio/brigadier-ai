//! The three plugin sign-in IPC operations, kept apart from the plugin page handlers.
use super::super::{
    SessionManager,
    mcp_login::{Open, Target},
    plugins::CatalogSignIn,
};
use crate::{Error, Result};
use brigadier_registry::{Catalog, Transport, catalog::Content};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

type SignInTarget = (String, Target, bool);
struct Targets {
    owner: Weak<super::McpGatewayService>,
    rows: HashMap<String, SignInTarget>,
}
static TARGETS: Mutex<Vec<Targets>> = Mutex::new(Vec::new());

pub(super) fn remember_targets(
    owner: &Arc<super::McpGatewayService>,
    rows: &[brigadier_registry::catalog::Row],
    data_dir: &std::path::Path,
) {
    let rows = rows
        .iter()
        .filter_map(|row| {
            row.definitions.iter().find_map(|definition| {
                if let Content::Server(server) = &definition.content
                    && server.signs_in
                    && let Transport::Http { url, .. } = &server.transport
                {
                    Some((
                        row.id.clone(),
                        (
                            url.clone(),
                            Target {
                                server: server.name.clone(),
                                plugin: server.plugin.as_ref().map(|plugin| plugin.name.clone()),
                                project: definition.provenance.project.clone(),
                            },
                            row.definitions.iter().all(|definition| {
                                definition
                                    .provenance
                                    .file
                                    .starts_with(data_dir.join("library"))
                            }),
                        ),
                    ))
                } else {
                    None
                }
            })
        })
        .collect();
    let mut targets = TARGETS.lock().unwrap_or_else(|p| p.into_inner());
    targets.retain(|target| {
        target.owner.strong_count() > 0 && !target.owner.ptr_eq(&Arc::downgrade(owner))
    });
    targets.push(Targets {
        owner: Arc::downgrade(owner),
        rows,
    });
}

pub(super) fn cached_target(
    owner: &Arc<super::McpGatewayService>,
    row_id: &str,
) -> Option<Result<SignInTarget>> {
    TARGETS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|target| target.owner.ptr_eq(&Arc::downgrade(owner)))
        .map(|target| {
            target.rows.get(row_id).cloned().ok_or_else(|| {
                Error::Invalid("Unknown plugin row or this row does not use remote OAuth".into())
            })
        })
}

impl SessionManager {
    /// Pass this authority to the launch resolver. Every endpoint call mints a new secret.
    pub fn mcp_gateway(&self) -> &Arc<super::McpGatewayService> {
        &self.mcp_gateway
    }

    /// Cache only sign-in routing from the catalog already read for the page or a launch.
    pub(crate) fn cache_plugin_sign_in_targets(&self, rows: &[brigadier_registry::catalog::Row]) {
        remember_targets(&self.mcp_gateway, rows, &self.data_dir);
    }

    async fn plugin_sign_in_target(&self, row_id: &str) -> Result<SignInTarget> {
        if let Some(target) = cached_target(&self.mcp_gateway, row_id) {
            return target;
        }
        let registry = self.registry();
        let projects = self.plugin_projects();
        let row_id = row_id.to_owned();
        let data_dir = self.data_dir.clone();
        let gateway = self.mcp_gateway.clone();
        super::super::blocking(move || {
            let projects: Vec<_> = projects.into_iter().map(|(_, _, path)| path).collect();
            let rows = Catalog::read_rows_with_library(registry.roots(), &projects, &data_dir);
            remember_targets(&gateway, &rows, &data_dir);
            cached_target(&gateway, &row_id).expect("catalog cached")
        })
        .await
    }

    pub async fn start_plugin_sign_in(&self, row_id: &str) -> Result<CatalogSignIn> {
        let started = std::time::Instant::now();
        tracing::info!("MCP plugin sign-in requested");
        let (resource, target, library_only) = self.plugin_sign_in_target(row_id).await?;
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "MCP plugin sign-in target resolved"
        );
        let open: Open = self
            .sign_in_browser
            .get()
            .cloned()
            .ok_or_else(|| Error::Invalid("Sign-in browser unavailable".into()))?;
        if matches!(
            self.mcp_gateway.status(&resource).await,
            CatalogSignIn::CliFallback { .. }
        ) {
            if library_only {
                return Ok(CatalogSignIn::CliSetupRequired {
                    agents: brigadier_providers::ProviderKind::ALL.to_vec(),
                });
            }
            self.mcp_logins.enable_account_spread(self.runtime.clone());
            let flow = self.mcp_logins.start(self.clis(), target, open);
            self.mcp_logins.set_fallback_flow(row_id, flow);
            return self.get_plugin_sign_in_status(row_id).await;
        }
        Ok(self.mcp_gateway.start_sign_in(&resource, open).await)
    }

    pub async fn get_plugin_sign_in_status(&self, row_id: &str) -> Result<CatalogSignIn> {
        let (resource, _, library_only) = self.plugin_sign_in_target(row_id).await?;
        let state = self.mcp_gateway.status(&resource).await;
        if library_only && matches!(state, CatalogSignIn::CliFallback { .. }) {
            return Ok(CatalogSignIn::CliSetupRequired {
                agents: brigadier_providers::ProviderKind::ALL.to_vec(),
            });
        }
        if matches!(state, CatalogSignIn::CliFallback { .. })
            && let Some(progress) = self.mcp_logins.fallback_progress(row_id)
        {
            if !progress.finished && progress.agents.len() < 2 {
                return Ok(state);
            }
            let agents = progress
                .agents
                .iter()
                .filter(|step| {
                    !matches!(
                        step.state,
                        super::super::mcp_login::AgentSignInState::SignedIn
                            | super::super::mcp_login::AgentSignInState::AlreadySignedIn
                    )
                })
                .map(|step| step.agent)
                .collect();
            return Ok(CatalogSignIn::CliFallback {
                agents,
                steps: progress.agents,
            });
        }
        Ok(state)
    }

    pub async fn sign_out_plugin(&self, row_id: &str) -> Result<()> {
        let (resource, target, library_only) = self.plugin_sign_in_target(row_id).await?;
        if matches!(
            self.mcp_gateway.status(&resource).await,
            CatalogSignIn::CliFallback { .. }
        ) && !library_only
        {
            let (logins, clis, runtime) =
                (self.mcp_logins.clone(), self.clis(), self.runtime.clone());
            let row_id = row_id.to_owned();
            let resource = resource.clone();
            super::super::blocking(move || {
                logins
                    .forget_fallback_accounts(&row_id, &target, &resource, &clis, &runtime)
                    .map_err(Error::Invalid)
            })
            .await?;
        }
        self.mcp_gateway
            .sign_out(&resource)
            .await
            .map_err(|_| Error::Invalid("Cannot remove gateway Keychain grant".into()))
    }
}
