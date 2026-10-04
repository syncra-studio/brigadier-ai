//! The registry lock also orders refresh admission, reset, commit and supersession.
use brigadier_providers::ProviderKind;
use brigadier_router::{MergedModel, RatingPatch, Registry, validate_patches};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use super::RegistryHolder;
use crate::now_ms;

/// The latest requested ranking refresh, from admission through completion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RankingsRefreshState {
    /// No refresh has been requested, or the completed refresh was reset.
    #[default]
    Idle,
    /// Checking for a newer published registry before capturing research inputs.
    CheckingRegistry,
    /// Researching the captured live catalog with read-only web tools.
    Researching,
    /// Valid sourced patches were saved for the effective registry revision.
    Done,
    /// Research, validation or persistence failed; the prior overlay is kept.
    Failed,
    /// A newer curated registry made this research inapplicable.
    Superseded,
    /// Reset or shutdown cancelled the running job.
    Cancelled,
}

/// An accepted model patch and the effective rating fields it changed.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RatingChange {
    /// The CLI provider that lists this model.
    pub provider: ProviderKind,
    /// The concrete model identity, never an inherited family registry key.
    pub model: String,
    /// Changed field paths, such as `strengths.review`; empty when values stayed the same.
    pub fields: Vec<String>,
    /// Research source URLs; at least one must be a parseable HTTPS URL.
    pub sources: Vec<String>,
}

/// Refresh progress and findings, with the saved overlay's current applicability.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RankingsRefresh {
    /// The latest job state.
    pub state: RankingsRefreshState,
    /// The admitted job id, or null when idle.
    pub job_id: Option<String>,
    /// When the job was admitted, in Unix milliseconds.
    pub started_at_ms: Option<i64>,
    /// When the job ended, in Unix milliseconds; null while running or idle.
    pub finished_at_ms: Option<i64>,
    /// The model producing the research, assigned after the registry check.
    pub model: Option<String>,
    /// The CLI provider running the research model.
    pub provider: Option<ProviderKind>,
    /// The host-captured curated revision researched by this job.
    pub base_revision: Option<u64>,
    /// Accepted model patches and their changed fields and sources.
    pub changes: Vec<RatingChange>,
    /// Research failures and reasons individual patches were dropped.
    pub errors: Vec<String>,
    /// Whether the saved overlay matches the effective curated revision.
    pub overlay_applied: bool,
    /// When the saved overlay was researched, even if superseded; null when absent.
    pub overlay_at_ms: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Overlay {
    pub base_revision: u64,
    pub at_ms: i64,
    pub patches: Vec<RatingPatch>,
    pub job: RankingsRefresh,
}

impl RegistryHolder {
    /// Read registry and overlay together so routing never mixes revisions.
    pub fn rating_snapshot(&self) -> (Arc<Registry>, Vec<RatingPatch>) {
        let state = self.state();
        let patches = state
            .overlay
            .as_ref()
            .filter(|o| o.base_revision == state.registry.revision)
            .map(|o| o.patches.clone())
            .unwrap_or_default();
        (state.registry.clone(), patches)
    }

    pub fn rankings_refresh(&self) -> RankingsRefresh {
        let state = self.state();
        let mut view = state.refresh.clone();
        view.overlay_applied = state
            .overlay
            .as_ref()
            .is_some_and(|o| o.base_revision == state.registry.revision);
        view.overlay_at_ms = state.overlay.as_ref().map(|o| o.at_ms);
        view
    }

    /// Admission is synchronous; the caller starts network work only for a new job.
    pub fn admit_refresh(&self) -> (String, Option<CancellationToken>) {
        let mut state = self.state();
        if let Some((id, _)) = &state.running {
            return (id.clone(), None);
        }
        let id = uuid::Uuid::now_v7().to_string();
        let cancel = CancellationToken::new();
        state.running = Some((id.clone(), cancel.clone()));
        state.refresh = RankingsRefresh {
            state: RankingsRefreshState::CheckingRegistry,
            job_id: Some(id.clone()),
            started_at_ms: Some(now_ms()),
            ..Default::default()
        };
        (id, Some(cancel))
    }

    /// Called after the initial repository check, immediately before the CLI research.
    pub fn capture_refresh(
        &self,
        id: &str,
        catalogs: &[(ProviderKind, &[brigadier_providers::ModelInfo])],
        producer: (ProviderKind, &str),
        research: &[brigadier_router::ResearchNote],
    ) -> Option<(u64, Vec<MergedModel>)> {
        let mut state = self.state();
        if !running(&state, id) {
            return None;
        }
        let revision = state.registry.revision;
        let mut models = brigadier_router::merge(&state.registry, catalogs, research, &[]);
        models.retain(|model| !model.excluded);
        if let Some(overlay) = &state.overlay
            && overlay.base_revision == revision
        {
            for model in &mut models {
                for patch in &overlay.patches {
                    patch.apply(model);
                }
            }
        }
        state.refresh.state = RankingsRefreshState::Researching;
        state.refresh.base_revision = Some(revision);
        state.refresh.provider = Some(producer.0);
        state.refresh.model = Some(producer.1.to_owned());
        Some((revision, models))
    }

    /// Validation and publication share the installation/reset lock. Invalid models keep
    /// their existing patches, including when the new result is entirely invalid.
    pub fn finish_refresh(
        &self,
        id: &str,
        base_revision: u64,
        models: &[MergedModel],
        reply: Result<String, String>,
    ) {
        let mut state = self.state();
        if !running(&state, id) {
            return;
        }
        let (patches, errors) = match reply {
            Ok(reply) if reply.len() <= super::MAX_BYTES => validate_patches(&reply, models),
            Ok(_) => (
                Vec::new(),
                vec!["research response exceeds the registry size limit".into()],
            ),
            Err(error) => (Vec::new(), vec![error]),
        };
        state.refresh.errors = errors;
        state.refresh.finished_at_ms = Some(now_ms());
        state.refresh.changes = patches
            .iter()
            .filter_map(|patch| {
                let model = models.iter().find(|model| patch.applies(model))?;
                Some(RatingChange {
                    provider: patch.provider,
                    model: patch.model.clone(),
                    fields: patch.changed_fields(model),
                    sources: patch.sources.clone(),
                })
            })
            .collect();
        state.refresh.state = if base_revision != state.registry.revision {
            RankingsRefreshState::Superseded
        } else if patches.is_empty() {
            RankingsRefreshState::Failed
        } else {
            RankingsRefreshState::Done
        };
        if !patches.is_empty() {
            let mut kept = state
                .overlay
                .as_ref()
                .filter(|o| o.base_revision == base_revision)
                .map(|o| o.patches.clone())
                .unwrap_or_default();
            for patch in patches {
                if let Some(prior) = kept
                    .iter_mut()
                    .find(|prior| prior.provider == patch.provider && prior.model == patch.model)
                {
                    prior.tier = patch.tier.or(prior.tier);
                    prior.strengths.extend(patch.strengths);
                    prior.area_strengths.extend(patch.area_strengths);
                    prior.default_effort.extend(patch.default_effort);
                    for source in patch.sources {
                        if !prior.sources.contains(&source) {
                            prior.sources.push(source);
                        }
                    }
                } else {
                    kept.push(patch);
                }
            }
            let overlay = Overlay {
                base_revision,
                at_ms: now_ms(),
                patches: kept,
                job: state.refresh.clone(),
            };
            let saved = serde_json::to_vec_pretty(&overlay)
                .map_err(std::io::Error::other)
                .and_then(|bytes| self.write_atomic("overlay.json", &bytes));
            match saved {
                Ok(()) => state.overlay = Some(overlay),
                Err(error) => {
                    state.refresh.state = RankingsRefreshState::Failed;
                    state
                        .refresh
                        .errors
                        .push(format!("overlay could not be saved: {error}"));
                }
            }
        }
        state.running = None;
    }

    pub fn cancel_refresh(&self) {
        let mut state = self.state();
        if let Some((_, cancel)) = state.running.take() {
            cancel.cancel();
            state.refresh.state = RankingsRefreshState::Cancelled;
            state.refresh.finished_at_ms = Some(now_ms());
        }
    }

    pub fn reset_rankings(&self) -> std::io::Result<()> {
        let mut state = self.state();
        if let Some((_, cancel)) = state.running.take() {
            cancel.cancel();
            state.refresh.state = RankingsRefreshState::Cancelled;
            state.refresh.finished_at_ms = Some(now_ms());
        } else {
            state.refresh = RankingsRefresh::default();
        }
        // A persisted null is an atomic reset, including when no overlay existed.
        self.write_atomic("overlay.json", b"null")?;
        state.overlay = None;
        Ok(())
    }
}

fn running(state: &super::State, id: &str) -> bool {
    state
        .running
        .as_ref()
        .is_some_and(|(running, cancel)| running == id && !cancel.is_cancelled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::Fetched;
    use std::path::PathBuf;

    struct Cache(PathBuf);
    impl Cache {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("rankings-{}", uuid::Uuid::now_v7())))
        }
        fn holder(&self) -> Arc<RegistryHolder> {
            RegistryHolder::load(&self.0)
        }
    }
    impl Drop for Cache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fetched(revision: u64) -> Fetched {
        let mut registry = Registry::bundled();
        registry.revision = revision;
        Fetched::Document {
            bytes: serde_json::to_vec(&registry).unwrap(),
            etag: Some(revision.to_string()),
            source: "fake repository".into(),
        }
    }
    fn capture(holder: &RegistryHolder, id: &str) -> (u64, Vec<MergedModel>) {
        let info = serde_json::from_value(serde_json::json!({
            "id":"gpt-6.1-sol", "displayName":"Sol", "description":"",
            "efforts":["low","medium","high"], "isDefault":true, "inputModalities":["text"], "legacy":false
        }))
        .unwrap();
        holder
            .capture_refresh(
                id,
                &[(ProviderKind::Codex, &[info])],
                (ProviderKind::Codex, "gpt-6.1-sol"),
                &[],
            )
            .unwrap()
    }
    fn reply() -> Result<String, String> {
        Ok(
            serde_json::json!({"patches":[{"provider":"codex","model":"gpt-6.1-sol",
            "sources":["https://example.com/model"],"strengths":{"research":9},
            "defaultEffort":{"research":"high"}}]})
            .to_string(),
        )
    }
    fn commit(holder: &RegistryHolder) {
        let (id, _) = holder.admit_refresh();
        let (revision, models) = capture(holder, &id);
        holder.finish_refresh(&id, revision, &models, reply());
        assert_eq!(holder.rankings_refresh().state, RankingsRefreshState::Done);
    }

    #[test]
    fn own_check_precedes_capture_restart_reset_and_invalid_results() {
        let cache = Cache::new();
        let holder = cache.holder();
        let (id, cancel) = holder.admit_refresh();
        let duplicate = holder.admit_refresh();
        assert_eq!(id, duplicate.0);
        assert!(duplicate.1.is_none());
        assert!(!cancel.unwrap().is_cancelled());
        let revision = holder.current().revision + 1;
        assert!(holder.take(fetched(revision)));
        let (base, models) = capture(&holder, &id);
        assert_eq!(base, revision);
        holder.finish_refresh(&id, base, &models, reply());
        assert!(holder.rankings_refresh().overlay_applied);
        let restarted = cache.holder();
        assert!(restarted.rankings_refresh().overlay_applied);
        assert_eq!(restarted.rating_snapshot().1, holder.rating_snapshot().1);
        let prior = holder.rating_snapshot().1;
        for reply in [
            Ok(r#"{"patches":[]}"#.into()),
            Ok("invalid".into()),
            Err("timeout".into()),
        ] {
            let (id, _) = holder.admit_refresh();
            let (base, models) = capture(&holder, &id);
            holder.finish_refresh(&id, base, &models, reply);
            assert_eq!(
                holder.rankings_refresh().state,
                RankingsRefreshState::Failed
            );
            assert_eq!(holder.rating_snapshot().1, prior);
        }
        holder.reset_rankings().unwrap();
        assert!(cache.holder().rating_snapshot().1.is_empty());
    }

    #[test]
    fn reset_wins_in_both_completion_orders() {
        for reset_first in [false, true] {
            let cache = Cache::new();
            let holder = cache.holder();
            commit(&holder);
            let (id, cancel) = holder.admit_refresh();
            let (base, models) = capture(&holder, &id);
            if reset_first {
                holder.reset_rankings().unwrap();
            }
            holder.finish_refresh(&id, base, &models, reply());
            if !reset_first {
                holder.reset_rankings().unwrap();
            }
            if reset_first {
                assert!(cancel.unwrap().is_cancelled());
            }
            assert!(!holder.rankings_refresh().overlay_applied);
            assert!(cache.holder().rating_snapshot().1.is_empty());
        }
    }

    #[test]
    fn interleaved_fetches_never_roll_back_memory_or_cache_and_supersede_overlay() {
        for newer_first in [false, true] {
            let cache = Cache::new();
            let holder = cache.holder();
            commit(&holder);
            let base = holder.current().revision;
            let (tx, rx) = std::sync::mpsc::channel();
            let pending = holder.clone();
            let delayed = if newer_first { base + 1 } else { base + 2 };
            let thread = std::thread::spawn(move || {
                let document = fetched(delayed);
                rx.recv().unwrap();
                pending.take(document)
            });
            holder.take(fetched(if newer_first { base + 2 } else { base + 1 }));
            tx.send(()).unwrap();
            thread.join().unwrap();
            assert_eq!(holder.current().revision, base + 2);
            assert_eq!(cache.holder().current().revision, base + 2);
            assert!(!holder.rankings_refresh().overlay_applied);
            assert_eq!(
                cache.holder().rankings_refresh().state,
                RankingsRefreshState::Superseded
            );
        }
    }

    #[test]
    fn upgrade_after_capture_commits_as_superseded_and_new_bundle_ignores_overlay() {
        let cache = Cache::new();
        let holder = cache.holder();
        let (id, _) = holder.admit_refresh();
        let (base, models) = capture(&holder, &id);
        holder.take(fetched(base + 1));
        holder.finish_refresh(&id, base, &models, reply());
        assert_eq!(
            holder.rankings_refresh().state,
            RankingsRefreshState::Superseded
        );
        assert!(!holder.rankings_refresh().overlay_applied);
        assert_eq!(
            cache.holder().rankings_refresh().state,
            RankingsRefreshState::Superseded
        );
        // Simulate a persisted overlay predating the bundle shipped by this app.
        let mut overlay = holder.state().overlay.clone().unwrap();
        overlay.base_revision = Registry::bundled().revision - 1;
        holder
            .write_atomic("overlay.json", &serde_json::to_vec(&overlay).unwrap())
            .unwrap();
        std::fs::remove_file(holder.dir.join(super::super::DOCUMENT)).unwrap();
        assert!(!cache.holder().rankings_refresh().overlay_applied);
    }
    #[test]
    fn installation_holds_the_lock_through_disk_write_and_publication() {
        let cache = Cache::new();
        let holder = cache.holder();
        commit(&holder);
        let base = holder.current().revision;
        let first = holder.clone();
        let (writing, started) = std::sync::mpsc::channel();
        let (release, proceed) = std::sync::mpsc::channel();
        let one = std::thread::spawn(move || {
            first.take_with(fetched(base + 1), |bytes, meta| {
                writing.send(()).unwrap();
                proceed.recv().unwrap();
                first.write(bytes, meta)
            })
        });
        started.recv().unwrap();
        assert!(matches!(
            holder.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let second = holder.clone();
        let two = std::thread::spawn(move || second.take(fetched(base + 2)));
        release.send(()).unwrap();
        one.join().unwrap();
        two.join().unwrap();
        assert_eq!(holder.current().revision, base + 2);
        assert_eq!(cache.holder().current().revision, base + 2);
        assert!(!holder.rankings_refresh().overlay_applied);
    }

    #[test]
    fn failed_cache_write_never_publishes_or_supersedes() {
        let cache = Cache::new();
        let holder = cache.holder();
        commit(&holder);
        let base = holder.current().revision;
        assert!(!holder.take_with(fetched(base + 1), |_, _| {
            Err(std::io::Error::other("fake disk failure"))
        }));
        assert_eq!(holder.current().revision, base);
        assert_eq!(cache.holder().current().revision, base);
        assert!(holder.rankings_refresh().overlay_applied);
    }
    #[test]
    fn a_partial_patch_keeps_previously_researched_fields() {
        let cache = Cache::new();
        let holder = cache.holder();
        commit(&holder);
        let (id, _) = holder.admit_refresh();
        let (base, models) = capture(&holder, &id);
        holder.finish_refresh(
            &id,
            base,
            &models,
            Ok(serde_json::json!({
                "patches":[{"provider":"codex", "model":"gpt-6.1-sol",
                    "sources":["https://example.com/new"], "tier":"light"}]
            })
            .to_string()),
        );
        let patches = holder.rating_snapshot().1;
        assert_eq!(patches[0].tier, Some(brigadier_router::QualityTier::Light));
        assert_eq!(
            patches[0].default_effort[&brigadier_router::TaskCategory::Research],
            "high"
        );
        assert_eq!(
            patches[0].strengths[&brigadier_router::TaskCategory::Research],
            9.0
        );
        assert_eq!(patches[0].sources.len(), 2);
    }
}
