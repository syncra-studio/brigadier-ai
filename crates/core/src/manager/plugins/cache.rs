//! Display snapshots only. Launch resolution and edits still read current configuration.
use super::{CatalogSignIn, CatalogView};
use crate::manager::SessionManager;
use std::{
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::watch;

const FILE: &str = "plugins-catalog.json";
const RECHECK: Duration = Duration::from_secs(30);

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    format: u8,
    catalog: CatalogView,
}

struct State {
    catalog: Option<CatalogView>,
    projects: Vec<std::path::PathBuf>,
    refreshing: bool,
    dirty: bool,
    generation: u64,
    checked: Option<Instant>,
    error: Option<String>,
}

pub(crate) struct Cache {
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

fn settled(catalog: &mut CatalogView) {
    for row in &mut catalog.rows {
        if matches!(row.sign_in, CatalogSignIn::SigningIn { .. }) {
            row.sign_in = CatalogSignIn::SignedOut;
        }
        if let CatalogSignIn::CliFallback { steps, .. } = &mut row.sign_in {
            for step in steps {
                if matches!(
                    step.state,
                    crate::manager::mcp_login::AgentSignInState::Waiting
                        | crate::manager::mcp_login::AgentSignInState::Starting
                        | crate::manager::mcp_login::AgentSignInState::InBrowser
                ) {
                    step.state = crate::manager::mcp_login::AgentSignInState::Cancelled;
                }
            }
        }
    }
}

fn version(catalog: &mut CatalogView) {
    catalog.version.clear();
    catalog.version = brigadier_registry::catalog::fingerprint(
        &serde_json::to_vec(catalog).expect("catalog serializes"),
    );
}

impl Cache {
    fn load(data_dir: &Path) -> Self {
        let catalog = std::fs::read(data_dir.join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
            .filter(|saved| saved.format == 1)
            .map(|mut saved| {
                settled(&mut saved.catalog);
                version(&mut saved.catalog);
                saved.catalog
            });
        Self {
            state: Mutex::new(State {
                catalog,
                projects: Vec::new(),
                refreshing: false,
                dirty: true,
                generation: 0,
                checked: None,
                error: None,
            }),
            changed: watch::channel(0).0,
        }
    }

    fn persist(data_dir: &Path, catalog: &CatalogView) {
        let mut catalog = catalog.clone();
        // A daemon restart cannot resume these flows; no transient flow identifiers go to disk.
        settled(&mut catalog);
        let result = serde_json::to_vec(&Saved { format: 1, catalog })
            .map_err(|error| crate::Error::Invalid(error.to_string()))
            .and_then(|bytes| {
                crate::manager::session_dir::write_atomic(&data_dir.join(FILE), &bytes)
            });
        if let Err(error) = result {
            tracing::warn!(reason = %error, "Cannot persist plugin catalog snapshot");
        }
    }

    fn update(&self, data_dir: &Path, change: impl FnOnce(&mut CatalogView)) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(catalog) = &mut state.catalog else {
            return;
        };
        let before = catalog.clone();
        change(catalog);
        if *catalog == before {
            return;
        }
        version(catalog);
        Self::persist(data_dir, catalog);
        state.generation += 1;
        self.changed.send_modify(|revision| *revision += 1);
    }
}

impl SessionManager {
    fn page_catalog_cache(&self) -> &Cache {
        self.plugin_catalog_cache
            .get_or_init(|| Cache::load(&self.data_dir))
    }

    pub(crate) fn invalidate_plugin_catalog(&self) {
        if let Some(cache) = self.plugin_catalog_cache.get() {
            let mut state = cache.state.lock().unwrap_or_else(|p| p.into_inner());
            state.generation += 1;
            state.dirty = true;
        }
    }

    pub(crate) fn cached_plugin_sign_in(&self, row_id: &str) -> Option<CatalogSignIn> {
        self.plugin_catalog_cache
            .get()?
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .catalog
            .as_ref()?
            .rows
            .iter()
            .find(|row| row.id == row_id)
            .map(|row| row.sign_in.clone())
    }

    pub(crate) fn cache_plugin_sign_in(
        &self,
        row_id: &str,
        sign_in: CatalogSignIn,
    ) -> CatalogSignIn {
        if let Some(cache) = self.plugin_catalog_cache.get() {
            cache.update(&self.data_dir, |view| {
                if let Some(row) = view.rows.iter_mut().find(|row| row.id == row_id) {
                    row.sign_in = sign_in.clone();
                }
            });
        }
        sign_in
    }

    pub(crate) fn cache_plugin_choice(&self, row_id: &str, choice: super::choices::Choice) {
        if let Some(cache) = self.plugin_catalog_cache.get() {
            cache.update(&self.data_dir, |view| {
                if let Some(row) = view.rows.iter_mut().find(|row| row.id == row_id) {
                    row.choice = choice;
                }
            });
        }
    }

    fn refresh_page_catalog(&self) {
        let cache = self.page_catalog_cache();
        let generation = {
            let mut state = cache.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.refreshing
                || (state.catalog.is_some()
                    && !state.dirty
                    && state.checked.is_some_and(|at| at.elapsed() < RECHECK))
            {
                return;
            }
            state.refreshing = true;
            state.error = None;
            state.generation
        };
        let Some(manager) = self.me.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            let reader = manager.clone();
            let result = crate::manager::blocking(move || reader.plugin_catalog()).await;
            let result = match result {
                Ok(mut view) => {
                    for row in &mut view.rows {
                        if !matches!(row.sign_in, CatalogSignIn::NotRequired)
                            && let Ok(state) = manager.get_plugin_sign_in_status(&row.id).await
                        {
                            row.sign_in = state;
                        }
                    }
                    version(&mut view);
                    Ok(view)
                }
                Err(error) => Err(error),
            };
            let cache = manager.page_catalog_cache();
            let mut state = cache.state.lock().unwrap_or_else(|p| p.into_inner());
            state.refreshing = false;
            state.checked = Some(Instant::now());
            if state.generation != generation {
                state.dirty = true;
            } else {
                state.dirty = false;
                match result {
                    Ok(view) => {
                        Cache::persist(&manager.data_dir, &view);
                        state.catalog = Some(view);
                    }
                    Err(error) => {
                        tracing::warn!(reason = %error, "Cannot refresh plugin catalog");
                        state.error = Some(error.to_string());
                    }
                }
            }
            cache.changed.send_modify(|revision| *revision += 1);
        });
    }

    /// Return the last page snapshot immediately; only a cache miss waits for discovery.
    pub async fn cached_plugin_catalog(&self) -> crate::Result<CatalogView> {
        self.read_page_catalog(false).await
    }

    /// Library mutations finish with a fresh display snapshot, before their acknowledgement.
    pub async fn rebuild_plugin_catalog(&self) -> crate::Result<()> {
        self.invalidate_plugin_catalog();
        self.read_page_catalog(true).await.map(|_| ())
    }

    async fn read_page_catalog(&self, fresh: bool) -> crate::Result<CatalogView> {
        let cache = self.page_catalog_cache();
        let mut projects: Vec<_> = self
            .plugin_projects()
            .into_iter()
            .map(|(_, _, path)| path)
            .collect();
        projects.sort();
        {
            let mut state = cache.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.projects != projects {
                state.projects = projects;
                state.generation += 1;
                state.dirty = true;
            }
        }
        let mut changed = cache.changed.subscribe();
        self.refresh_page_catalog();
        loop {
            {
                let state = cache.state.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(view) = &state.catalog
                    && (!fresh || (!state.dirty && !state.refreshing))
                {
                    return Ok(view.clone());
                }
                if !state.refreshing
                    && let Some(error) = &state.error
                {
                    return Err(crate::Error::Invalid(error.clone()));
                }
            }
            self.refresh_page_catalog();
            changed
                .changed()
                .await
                .map_err(|_| crate::Error::Invalid("Plugin catalog refresh stopped".into()))?;
        }
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
