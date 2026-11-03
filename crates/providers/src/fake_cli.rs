//! A throwaway fourth CLI, for tests only (feature `fake-cli`): it proves a new CLI is one
//! adapter module and one line in [`crate::adapters::ADAPTERS`]. Its binary is never found, so
//! a real runtime reads it as not installed; tests drive its kind with a scripted stand-in.

use std::sync::Arc;

use crate::model::*;
use crate::{BoxFuture, Error, Ledger, Provider, Replayer, Result, Started, record};

pub static ADAPTER: crate::adapters::Adapter = crate::adapters::Adapter {
    kind: ProviderKind::new("fakecli"),
    label: "Fake CLI",
    binary: "brigadier-fake-cli",
    install: ["", ""],
    login: "",
    sign_in: &[],
    sign_out: &[],
    home: None,
    tools: &[],
    session_files: &[],
    vendor: "Fake",
    maker: "Fake",
    cli_name: "Fake CLI",
    install_guide: "",
    usual_windows: &[],
    glyph: crate::adapters::Glyph {
        view_box: "0 0 1 1",
        path: "M0 0H1V1H0Z",
        class_name: None,
    },
    one_skill_per_message: false,
    reports_cache_writes: false,
    commands_as_text: true,
    subagent_tools: &[],
    builtin_commands: &[],
    mcp_prompts: false,
    cumulative_usage: false,
    chore_family: "",
    chore_effort: None,
    coauthors_in_instructions: false,
    compaction: None,
    brigadier_compacts: false,
    subagent_transcripts: false,
    output_hook: false,
    child_usage: None,
    removes_by_id: false,
    sign_in_code_page: false,
    installer_off_path: false,
    space_files: &[],
    temp_dir: None,
    npm_package: "",
    update_limits: |_| crate::adapters::UpdateLimits::default(),
    skills_overlay: |_, _| Ok(()),
    new: |_, _| Arc::new(FakeCli),
    names_itself: |_| false,
    installer_folder: |_| None,
    login_mark: |_| None,
    mcp_stores: |_, _| Vec::new(),
    read_session: |_| Ok(Vec::new()),
    item_sources: crate::items::own_file,
    items: |_, display, describe| crate::items::from_display(display, describe),
    session_context: |_| Ok(None),
    follow_session: |_| Box::new(|| Ok(Vec::new())),
    moved_session_file: |_| None,
    leftovers: |_, _, _| Vec::new(),
    delta_location: |_| None,
    stream_end: |_| None,
    usage_limit: |_| Vec::new(),
    trust: None,
};

/// Never installed: every call that would run it fails.
struct FakeCli;

impl Provider for FakeCli {
    fn kind(&self) -> ProviderKind {
        ADAPTER.kind
    }

    fn status(&self) -> BoxFuture<'_, ProviderStatus> {
        Box::pin(async {
            ProviderStatus {
                provider: ADAPTER.kind,
                path: None,
                version: None,
                logged_in: false,
                auth_method: None,
                plan: None,
                email: None,
                organization: None,
                guidance: Some("Fake CLI isn't installed.".into()),
                compacts: false,
            }
        })
    }

    fn models(&self) -> BoxFuture<'_, Result<ModelCatalog>> {
        Box::pin(async { Err(Error::NotInstalled(ADAPTER.kind)) })
    }

    fn quota(&self) -> BoxFuture<'_, Result<QuotaSnapshot>> {
        Box::pin(async { Err(Error::NotInstalled(ADAPTER.kind)) })
    }

    fn start(&self, _: SessionSpec, _: Arc<dyn Ledger>) -> BoxFuture<'_, Result<Started>> {
        Box::pin(async { Err(Error::NotInstalled(ADAPTER.kind)) })
    }

    fn remove(&self, _: Vec<Artifact>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn replayer(&self) -> Box<dyn Replayer> {
        Box::new(FakeCli)
    }
}

impl Replayer for FakeCli {
    fn feed(&mut self, _: record::Direction, _: &str) -> Vec<ProviderEvent> {
        Vec::new()
    }
}
