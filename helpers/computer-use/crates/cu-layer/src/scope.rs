//! What the caller of one tool call is allowed, carried to the guards.
//!
//! The daemon builds the trusted scope of the agent's session (never from a
//! tool argument) and runs the registry call inside [`with_scope`]. Every
//! guarded tool that call reaches, including each `run_steps` step and the
//! registry's own re-routes, reads it with [`current`].

use std::sync::Arc;

use crate::target_policy::TargetIdentity;

/// Called with each guarded call's tool and the app it acts on (`None` when
/// it names no app). Returns the refusal message when the user told agents
/// to stop using that app.
pub type TargetHook = Arc<dyn Fn(&str, Option<&TargetIdentity>) -> Option<String> + Send + Sync>;

/// The trusted scope of one tool call.
#[derive(Clone, Default)]
pub struct DispatchScope {
    /// Exact bundle ids (lower case) this session may target although the
    /// default policy refuses them, e.g. a terminal or a Brigadier dev build.
    pub allowed_bundles: Vec<String>,
    /// Notes the app each call acts on and refuses apps the user stopped.
    pub on_target: Option<TargetHook>,
    /// The agent session's key, as session-end hooks get it; `None` when
    /// the call has no session (one-shot commands).
    pub session: Option<String>,
}

impl std::fmt::Debug for DispatchScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchScope")
            .field("allowed_bundles", &self.allowed_bundles)
            .field("on_target", &self.on_target.is_some())
            .field("session", &self.session)
            .finish()
    }
}

tokio::task_local! {
    static SCOPE: DispatchScope;
}

/// Run `f` with `scope` as the scope of every guarded call it makes.
pub async fn with_scope<F: std::future::Future>(scope: DispatchScope, f: F) -> F::Output {
    SCOPE.scope(scope, f).await
}

/// The scope of the running call; the default (nothing unlocked, nothing
/// noted) outside one.
pub fn current() -> DispatchScope {
    SCOPE.try_with(Clone::clone).unwrap_or_default()
}
