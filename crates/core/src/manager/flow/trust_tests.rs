//! "Do you trust this folder?" ([`crate::manager::trust`]): the answer reaches both CLIs'
//! settings and is undone exactly; Don't trust holds the folder's sessions to Ask for approval.
//! Every test writes a scratch home of its own, never the user's files.

use std::path::{Path, PathBuf};

use brigadier_providers::Artifact;
use brigadier_providers::trust::{TrustCli, plan};

use super::{Flow, Options};
use crate::model::{PermissionLevel, ProjectId};

pub(super) const CLAUDE: &str = "{\n  \"numStartups\": 3,\n  \"projects\": {\n    \"/elsewhere\": {\n      \"hasTrustDialogAccepted\": true\n    }\n  }\n}";
pub(super) const CODEX: &str =
    "model = \"gpt-6-astra\"\n\n[projects.\"/elsewhere\"]\ntrust_level = \"trusted\"\n";

/// A scratch home with both CLIs set up, removed when dropped.
pub(super) struct Home(pub PathBuf);

impl Home {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-trust-home-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        std::fs::write(dir.join(".claude.json"), CLAUDE).unwrap();
        std::fs::write(dir.join(".codex/config.toml"), CODEX).unwrap();
        Self(dir)
    }

    pub fn file(&self, cli: TrustCli) -> PathBuf {
        match cli {
            TrustCli::Claude => self.0.join(".claude.json"),
            TrustCli::Codex => self.0.join(".codex/config.toml"),
        }
    }

    pub fn text(&self, cli: TrustCli) -> String {
        std::fs::read_to_string(self.file(cli)).unwrap()
    }

    /// Whether `cli` trusts `folder` now.
    pub fn trusts(&self, cli: TrustCli, folder: &Path) -> bool {
        plan(cli, &self.file(cli), &folder.display().to_string())
            .unwrap()
            .is_none()
    }

    /// The manager writes here.
    pub fn serve(&self, flow: &Flow) {
        flow.manager.trust_home.set(self.0.clone()).unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project_of(flow: &Flow) -> ProjectId {
    flow.core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap()
}

fn trust_records(flow: &Flow, project: &ProjectId) -> Vec<Artifact> {
    flow.manager
        .runtime
        .ledger()
        .artifacts(&format!("trust:{project}"))
}

const BOTH: [TrustCli; 2] = [TrustCli::Claude, TrustCli::Codex];

/// Trust writes both CLIs' entries for the repository, recorded first; Don't trust and the
/// project's removal put both files back byte for byte.
#[tokio::test]
async fn a_trusted_folder_is_answered_for_both_clis_until_untrusted_or_removed() {
    let flow = Flow::start("trust-both", Options::default(), super::no_findings()).await;
    let home = Home::new();
    home.serve(&flow);
    let project = project_of(&flow);

    let report = flow
        .manager
        .set_folder_trust(project.clone(), None, true)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    let repo = flow.repo.display().to_string();
    assert_eq!(report.project.trusts(&repo), Some(true));
    for cli in BOTH {
        assert!(home.trusts(cli, &flow.repo), "{cli:?}");
        assert!(home.trusts(cli, Path::new("/elsewhere")), "{cli:?}");
    }
    assert_eq!(trust_records(&flow, &project).len(), 2);
    assert_eq!(
        flow.manager.permission(&flow.conversation),
        PermissionLevel::FullAccess
    );

    let report = flow
        .manager
        .set_folder_trust(project.clone(), Some(repo.clone()), false)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(home.text(TrustCli::Claude), CLAUDE);
    assert_eq!(home.text(TrustCli::Codex), CODEX);
    assert!(trust_records(&flow, &project).is_empty());
    assert_eq!(
        flow.manager.permission(&flow.conversation),
        PermissionLevel::AskForApproval
    );

    flow.manager
        .set_folder_trust(project.clone(), None, true)
        .await
        .unwrap();
    assert!(home.trusts(TrustCli::Codex, &flow.repo));
    let removed = flow
        .manager
        .remove_project(project.clone(), Vec::new(), true)
        .await
        .unwrap();
    assert!(removed.failures.is_empty(), "{:?}", removed.failures);
    assert_eq!(home.text(TrustCli::Claude), CLAUDE);
    assert_eq!(home.text(TrustCli::Codex), CODEX);
    assert!(trust_records(&flow, &project).is_empty());
    flow.stop().await;
}

/// A folder the user trusted in a CLI themselves stays trusted there whatever the answer;
/// another folder's answer or a CLI not set up here touches nothing.
#[tokio::test]
async fn trust_the_user_gave_a_cli_is_never_recorded_or_removed() {
    let flow = Flow::start("trust-theirs", Options::default(), super::no_findings()).await;
    let home = Home::new();
    let repo = flow.repo.display().to_string();
    let theirs = CLAUDE.replace("/elsewhere", &repo);
    std::fs::write(home.file(TrustCli::Claude), &theirs).unwrap();
    // Codex isn't set up here.
    std::fs::remove_dir_all(home.0.join(".codex")).unwrap();
    home.serve(&flow);
    let project = project_of(&flow);

    flow.manager
        .set_folder_trust(project.clone(), None, true)
        .await
        .unwrap();
    assert!(trust_records(&flow, &project).is_empty());
    assert!(!home.0.join(".codex").exists());
    flow.manager
        .set_folder_trust(project.clone(), None, false)
        .await
        .unwrap();
    assert_eq!(home.text(TrustCli::Claude), theirs);
    flow.stop().await;
}

/// A write cut short (recorded, not written) is finished at the next start while the folder is
/// still trusted; an entry no answer stands behind any more is put back.
#[tokio::test]
async fn a_restart_finishes_or_undoes_folder_trust_by_the_answers() {
    let flow = Flow::start("trust-reconcile", Options::default(), super::no_findings()).await;
    let home = Home::new();
    home.serve(&flow);
    let project = project_of(&flow);
    let repo = flow.repo.display().to_string();
    let owner = format!("trust:{project}");
    let ledger = flow.manager.runtime.ledger().clone();

    // Trusted, and the write never happened.
    flow.core
        .set_folder_trust(&project, &repo, true)
        .await
        .unwrap();
    ledger
        .record(
            &owner,
            Artifact::CliTrust {
                cli: TrustCli::Codex,
                file: home.file(TrustCli::Codex).display().to_string(),
                folder: repo.clone(),
                before: brigadier_providers::trust::TrustBefore::NoEntry,
            },
        )
        .await
        .unwrap();
    flow.manager.reconcile_trust().await;
    for cli in BOTH {
        assert!(home.trusts(cli, &flow.repo), "{cli:?}");
    }
    assert_eq!(ledger.artifacts(&owner).len(), 2);

    // The answer changed while the daemon was down.
    flow.core
        .set_folder_trust(&project, &repo, false)
        .await
        .unwrap();
    flow.manager.reconcile_trust().await;
    assert_eq!(home.text(TrustCli::Claude), CLAUDE);
    assert_eq!(home.text(TrustCli::Codex), CODEX);
    assert!(ledger.artifacts(&owner).is_empty());
    flow.stop().await;
}

/// Tests that set no home write nothing at all.
#[tokio::test]
async fn without_a_scratch_home_tests_write_no_cli_settings() {
    let flow = Flow::start("trust-nowhere", Options::default(), super::no_findings()).await;
    let project = project_of(&flow);
    let report = flow
        .manager
        .set_folder_trust(project.clone(), None, true)
        .await
        .unwrap();
    assert!(report.failures.is_empty());
    assert!(trust_records(&flow, &project).is_empty());
    flow.stop().await;
}
