//! The model registry in use (PLAN.md §6 Phase 5): the copy built into the app, or a newer one
//! downloaded from the repository and cached in `<data>/cache/registry/`.
//!
//! A downloaded copy goes through the same checks as the bundled one
//! ([`brigadier_router::Registry::parse`]: size cap, schema version, bounds) and is taken only
//! when its revision is above the one in use, so an older or replayed copy can never roll the
//! registry back. It is written atomically next to a `meta.json` (its ETag, revision and when
//! it was fetched). At launch the cache is read again and used only while it is newer than the
//! bundled copy, so a newer app always beats an older cache.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use brigadier_router::{Registry, RegistryInfo, RegistrySource};
use serde::{Deserialize, Serialize};

use crate::now_ms;

mod refresh;
pub use refresh::{RankingsRefresh, RankingsRefreshState, RatingChange};

/// The largest registry document read.
pub const MAX_BYTES: usize = brigadier_router::MAX_REGISTRY_BYTES;

const DOCUMENT: &str = "models.json";
const META: &str = "meta.json";

/// What the cached download says about itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    etag: Option<String>,
    revision: u64,
    fetched_at_ms: i64,
    source: String,
}

struct State {
    registry: Arc<Registry>,
    source: RegistrySource,
    etag: Option<String>,
    fetched_at_ms: Option<i64>,
    checked_at_ms: Option<i64>,
    error: Option<String>,
    overlay: Option<refresh::Overlay>,
    refresh: RankingsRefresh,
    running: Option<(String, tokio_util::sync::CancellationToken)>,
}

/// What asking the repository for the registry gave.
pub enum Fetched {
    /// The copy the ETag names is still current.
    NotModified,
    /// A document (not yet checked).
    Document {
        bytes: Vec<u8>,
        etag: Option<String>,
        source: String,
    },
    /// The request failed.
    Failed(String),
}

pub struct RegistryHolder {
    dir: PathBuf,
    state: Mutex<State>,
}

impl RegistryHolder {
    /// The registry to use at launch: the cached download if it still reads and is newer than
    /// the bundled copy, else the bundled one. Blocking (reads the cache).
    pub fn load(cache_dir: &Path) -> Arc<Self> {
        let dir = cache_dir.join("registry");
        let meta: Option<Meta> = std::fs::read(dir.join(META))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let cached = std::fs::read(dir.join(DOCUMENT))
            .ok()
            .and_then(|bytes| match Registry::parse(&bytes) {
                Ok(registry) => Some(registry),
                Err(err) => {
                    tracing::warn!(error = %err, "the cached model registry doesn't read; using the bundled one");
                    None
                }
            });
        let (registry, source) = Registry::effective(cached);
        // The meta speaks for the copy in use only if that is the cached download it describes:
        // otherwise its ETag could get a 304 for a copy that isn't in use.
        let meta = meta.filter(|meta| {
            source == RegistrySource::Downloaded && meta.revision == registry.revision
        });
        let overlay: Option<refresh::Overlay> = std::fs::read(dir.join("overlay.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let mut refresh = overlay
            .as_ref()
            .map(|overlay| overlay.job.clone())
            .unwrap_or_default();
        if let Some(overlay) = &overlay
            && overlay.base_revision != registry.revision
        {
            refresh.state = RankingsRefreshState::Superseded;
        }
        let fetched_at_ms = meta.as_ref().map(|meta| meta.fetched_at_ms);
        tracing::info!(
            revision = registry.revision,
            ?source,
            "model registry loaded"
        );
        Arc::new(Self {
            dir,
            state: Mutex::new(State {
                registry: Arc::new(registry),
                source,
                etag: meta.and_then(|meta| meta.etag),
                fetched_at_ms,
                checked_at_ms: None,
                error: None,
                overlay,
                refresh,
                running: None,
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The registry in use.
    pub fn current(&self) -> Arc<Registry> {
        self.state().registry.clone()
    }

    /// The Usage page's summary.
    pub fn info(&self) -> RegistryInfo {
        let state = self.state();
        state.registry.info(
            state.source,
            state.fetched_at_ms,
            state.checked_at_ms,
            state.error.clone(),
        )
    }

    /// The ETag of the cached download, for a conditional request.
    pub fn etag(&self) -> Option<String> {
        self.state().etag.clone()
    }

    /// Takes in what the repository answered. Answers whether the registry in use changed.
    /// Blocking (writes the cache).
    pub fn take(&self, fetched: Fetched) -> bool {
        self.take_with(fetched, |bytes, meta| self.write(bytes, meta))
    }

    fn take_with(
        &self,
        fetched: Fetched,
        write: impl FnOnce(&[u8], &Meta) -> std::io::Result<()>,
    ) -> bool {
        let now = now_ms();
        let (bytes, etag, source) = match fetched {
            Fetched::NotModified => {
                let mut state = self.state();
                state.checked_at_ms = Some(now);
                state.error = None;
                return false;
            }
            Fetched::Failed(error) => {
                let mut state = self.state();
                state.checked_at_ms = Some(now);
                state.error = Some(error);
                return false;
            }
            Fetched::Document {
                bytes,
                etag,
                source,
            } => (bytes, etag, source),
        };
        let refused = |state: &mut State, error: String| {
            tracing::warn!(error = %error, "a downloaded model registry was refused");
            state.checked_at_ms = Some(now);
            state.error = Some(error);
            false
        };
        let registry = match Registry::parse(&bytes) {
            Ok(registry) => registry,
            Err(err) => return refused(&mut self.state(), format!("it doesn't read: {err}")),
        };
        let mut state = self.state();
        let current = state.registry.clone();
        if !registry.is_newer_than(&current) {
            if registry.revision == current.revision {
                // The same revision under a new ETag: nothing to take, nothing wrong.
                state.checked_at_ms = Some(now);
                state.error = None;
                state.etag = etag;
                return false;
            }
            return refused(
                &mut state,
                format!(
                    "its revision {} is older than the one in use ({})",
                    registry.revision, current.revision
                ),
            );
        }
        let meta = Meta {
            etag: etag.clone(),
            revision: registry.revision,
            fetched_at_ms: now,
            source,
        };
        if let Err(err) = write(&bytes, &meta) {
            return refused(&mut state, format!("it could not be saved: {err}"));
        }
        tracing::info!(revision = registry.revision, "model registry updated");
        state.registry = Arc::new(registry);
        if state.overlay.is_some() && state.running.is_none() {
            state.refresh.state = RankingsRefreshState::Superseded;
        }
        state.source = RegistrySource::Downloaded;
        state.etag = etag;
        state.fetched_at_ms = Some(now);
        state.checked_at_ms = Some(now);
        state.error = None;
        true
    }

    /// Writes the document and its meta atomically (a temporary file renamed into place).
    fn write(&self, bytes: &[u8], meta: &Meta) -> std::io::Result<()> {
        let meta = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
        // Publish the document last. If either write fails, the old document still wins;
        // an unmatched meta is ignored at startup, so it cannot supply a stale ETag.
        self.write_atomic(META, &meta)?;
        self.write_atomic(DOCUMENT, bytes)
    }

    fn write_atomic(&self, name: &str, contents: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir)?;
        let temporary = self
            .dir
            .join(format!(".{name}.{}.tmp", uuid::Uuid::now_v7()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(contents)?;
            file.sync_all()?;
            std::fs::rename(&temporary, self.dir.join(name))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}
