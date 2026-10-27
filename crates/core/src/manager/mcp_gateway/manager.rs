//! The three plugin sign-in IPC operations, kept apart from the plugin page handlers.
use super::super::{
    SessionManager,
    mcp_login::{Open, Target},
    plugins::CatalogSignIn,
};
use crate::{Error, Result};
use brigadier_registry::{Catalog, Transport, catalog::Content};
use std::sync::Arc;

impl SessionManager {
    /// Pass this authority to the launch resolver. Every endpoint call mints a new secret.
    pub fn mcp_gateway(&self) -> &Arc<super::McpGatewayService> {
        &self.mcp_gateway
    }

    async fn plugin_sign_in_target(&self, row_id: &str) -> Result<(String, Target)> {
        let registry = self.registry();
        let projects = self.plugin_projects();
        let row_id = row_id.to_owned();
        let data_dir = self.data_dir.clone();
        super::super::blocking(move || {
            let projects: Vec<_> = projects.into_iter().map(|(_, _, path)| path).collect();
            let rows = Catalog::read_rows_with_library(registry.roots(), &projects, &data_dir);
            let row = rows
                .iter()
                .find(|row| row.id == row_id)
                .ok_or_else(|| Error::Invalid("Unknown plugin row".into()))?;
            for definition in &row.definitions {
                if let Content::Server(server) = &definition.content
                    && server.signs_in
                    && let Transport::Http { url, .. } = &server.transport
                {
                    return Ok((
                        url.clone(),
                        Target {
                            server: server.name.clone(),
                            plugin: server.plugin.as_ref().map(|plugin| plugin.name.clone()),
                            project: definition.provenance.project.clone(),
                        },
                    ));
                }
            }
            Err(Error::Invalid("This row does not use remote OAuth".into()))
        })
        .await
    }

    pub async fn start_plugin_sign_in(&self, row_id: &str) -> Result<CatalogSignIn> {
        let (resource, target) = self.plugin_sign_in_target(row_id).await?;
        let open: Open = self
            .sign_in_browser
            .get()
            .cloned()
            .ok_or_else(|| Error::Invalid("Sign-in browser unavailable".into()))?;
        if matches!(
            self.mcp_gateway.status(&resource).await,
            CatalogSignIn::CliFallback { .. }
        ) {
            self.mcp_logins.enable_account_spread(self.runtime.clone());
            let flow = self.mcp_logins.start(self.clis(), target, open);
            self.mcp_logins.set_fallback_flow(row_id, flow);
            return self.get_plugin_sign_in_status(row_id).await;
        }
        Ok(self.mcp_gateway.start_sign_in(&resource, open).await)
    }

    pub async fn get_plugin_sign_in_status(&self, row_id: &str) -> Result<CatalogSignIn> {
        let (resource, _) = self.plugin_sign_in_target(row_id).await?;
        let state = self.mcp_gateway.status(&resource).await;
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
            return Ok(CatalogSignIn::CliFallback { agents });
        }
        Ok(state)
    }

    pub async fn sign_out_plugin(&self, row_id: &str) -> Result<()> {
        let (resource, target) = self.plugin_sign_in_target(row_id).await?;
        if matches!(
            self.mcp_gateway.status(&resource).await,
            CatalogSignIn::CliFallback { .. }
        ) {
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
