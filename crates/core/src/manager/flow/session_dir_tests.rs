//! One folder per session (S7, G13): a session's CLI works and keeps its temporary files in
//! its own folder, the folder records the environment chosen on the rail, Archive keeps its
//! records, Delete removes it all, and a delete a crash cut off finishes at the next start.

use std::path::Path;
use std::sync::Arc;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{
    ConversationKind, Environment, EnvironmentRequest, ModelChoice, PermissionLevel, Setup,
    SetupRequest,
};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn workspace_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A New worktree session's CLI works in its folder's `work/` with `TMPDIR` in its `tmp/`;
/// `record/workspace.json` and git config name the base chosen on the rail. The folder lies
/// outside the data folder.
#[tokio::test]
async fn a_new_worktree_session_lives_in_its_own_folder() {
    let flow = Flow::start(
        "session-dir-worktree",
        Options::default(),
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    let dir = flow.manager.session_dir(&flow.conversation);
    assert_eq!(
        dir.root().file_name().unwrap().to_str().unwrap(),
        flow.conversation.short()
    );
    assert!(!dir.root().starts_with(flow.dir.join("data")));
    let specs = flow.thread_specs();
    let (_, spec) = specs.last().expect("the thread's CLI");
    assert_eq!(spec.cwd, dir.work());
    let env: std::collections::HashMap<_, _> = spec.env.iter().cloned().collect();
    assert_eq!(env.get("TMPDIR").map(String::as_str), dir.tmp().to_str());
    assert!(
        env.get("TMPPREFIX")
            .is_some_and(|p| Path::new(p).starts_with(dir.tmp()))
    );
    assert!(dir.tmp().is_dir());
    let branch = match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment: Environment::NewWorktree { branch, path, .. },
            ..
        }) => {
            assert_eq!(path.as_deref().map(Path::new), Some(dir.work().as_path()));
            branch
        }
        other => panic!("not a New worktree session: {other:?}"),
    };
    let record = workspace_json(&dir.record().join("workspace.json"));
    assert_eq!(record["environment"], "newWorktree");
    assert_eq!(record["base"], "main");
    assert_eq!(record["branch"], branch.as_str());
    assert_eq!(
        git_out(
            &flow.repo,
            &["config", "--get", &format!("branch.{branch}.gh-merge-base")]
        ),
        "main"
    );
    flow.stop().await;
}

/// A Local checkout session works in the user's own checkout, never in its folder's `work/`;
/// its record names its branch.
#[tokio::test]
async fn a_local_checkout_session_works_in_the_checkout_and_records_its_branch() {
    let mut flow = Flow::start(
        "session-dir-local",
        Options::default(),
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    git(&flow.repo, &["branch", "dev"]);
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    flow.conversation = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            project,
            Some("Local".into()),
            Some(SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: EnvironmentRequest::LocalCheckout {
                    branch: "dev".into(),
                    create_from: None,
                },
                permission: PermissionLevel::Full,
                orchestrator: ModelChoice {
                    provider: brigadier_providers::ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap()
        .id;
    flow.say("Look around.").await;
    flow.settled().await;
    let dir = flow.manager.session_dir(&flow.conversation);
    let specs = flow.thread_specs();
    let (_, spec) = specs.last().expect("the thread's CLI");
    assert_eq!(spec.cwd, flow.repo);
    let env: std::collections::HashMap<_, _> = spec.env.iter().cloned().collect();
    assert_eq!(env.get("TMPDIR").map(String::as_str), dir.tmp().to_str());
    assert!(!dir.work().exists());
    let record = workspace_json(&dir.record().join("workspace.json"));
    assert_eq!(record["environment"], "localCheckout");
    assert_eq!(record["branch"], "dev");
    assert!(record["base"].is_null());
    flow.stop().await;
}

/// Archive removes what the session made but its records; Delete removes the folder.
#[tokio::test]
async fn archive_keeps_the_record_and_delete_removes_the_folder() {
    let flow = Flow::start(
        "session-dir-archive",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Work.").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    let dir = flow.manager.session_dir(&id);
    std::fs::write(dir.tmp().join("scratch.txt"), "x").unwrap();
    assert!(dir.work().join(".git").is_file());
    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    super::eventually_async("the archive's cleanup to finish", || async {
        !flow.core.conversation(&id).unwrap().cleanup_pending
    })
    .await;
    assert!(!dir.tmp().exists(), "tmp/ goes with the archive");
    assert!(!dir.work().exists(), "the worktree goes with the archive");
    assert!(
        dir.record().join("workspace.json").is_file(),
        "the record stays"
    );
    assert!(
        !flow
            .manager
            .runtime
            .ledger()
            .artifacts(&crate::manager::session_dir::folder_owner(&id))
            .is_empty()
    );
    flow.manager.delete(id.clone()).await.unwrap();
    super::eventually("the session to go", || flow.core.conversation(&id).is_err()).await;
    flow.manager.cleanup_finished(&id).await;
    assert!(!dir.root().exists(), "Delete removes the whole folder");
    assert!(
        flow.manager
            .runtime
            .ledger()
            .artifacts(&crate::manager::session_dir::folder_owner(&id))
            .is_empty()
    );
    flow.stop().await;
}

/// A delete whose folder couldn't go is finished by the next start's sweep.
#[cfg(unix)]
#[tokio::test]
async fn a_delete_cut_off_removes_the_folder_at_the_next_start() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut flow = Flow::start(
        "session-dir-crash",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Work.").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    let dir = flow.manager.session_dir(&id);
    // A record the delete can't remove yet: its folder is read-only.
    let held = dir.record().join("held");
    std::fs::create_dir_all(&held).unwrap();
    std::fs::write(held.join("note.md"), "kept").unwrap();
    std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o500)).unwrap();
    flow.manager.delete(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(dir.root().exists(), "the folder stays while it can't go");
    let owner = crate::manager::session_dir::folder_owner(&id);
    assert!(flow.manager.runtime.ledger().disposing().contains(&owner));
    // Still marked as being deleted (hidden), so the next start finishes it (D4).
    assert!(flow.core.conversation(&id).is_ok_and(|c| c.deleting));
    std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o700)).unwrap();
    flow.crash().await;
    super::eventually("the session to go", || flow.core.conversation(&id).is_err()).await;
    assert!(!dir.root().exists(), "the next start finishes the delete");
    assert!(flow.manager.runtime.ledger().artifacts(&owner).is_empty());
    flow.stop().await;
}

/// A Unix socket made in a session's `TMPDIR` binds: the folder's path is short enough even
/// for a dev build's (`~/.brigadier/<instance>/s/<id>/tmp`), with Python's own temp names.
#[cfg(target_os = "macos")]
#[test]
fn a_unix_socket_binds_under_the_session_tmpdir() {
    let python = std::path::PathBuf::from("/usr/bin/python3");
    if !python.exists() {
        return;
    }
    // As long as the longest real one: `<home>/.brigadier/<12 hex>/s/<8>/tmp`.
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let real = home
        .join(".brigadier")
        .join("0123456789ab")
        .join("s")
        .join("abcdefgh")
        .join("tmp");
    let root = Path::new("/tmp").join(format!(
        "brigadier-socket-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let pad = real
        .as_os_str()
        .len()
        .saturating_sub(root.as_os_str().len() + 1);
    let tmp = root.join("x".repeat(pad.max(1)));
    std::fs::create_dir_all(&tmp).unwrap();
    assert!(tmp.as_os_str().len() >= real.as_os_str().len());
    let out = std::process::Command::new(python)
        .args([
            "-c",
            "import os, socket, tempfile\n\
             d = tempfile.mkdtemp(prefix='pymp-')\n\
             s = socket.socket(socket.AF_UNIX)\n\
             s.bind(os.path.join(d, 'listener-' + 'q' * 8))\n\
             print(len(s.getsockname()))",
        ])
        .env("TMPDIR", &tmp)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The CLI's files are the record (S8): Archive keeps the thread's CLI session file and
/// `record/lineage.json`, Restore resumes that same CLI session, and Delete removes both.
#[tokio::test]
async fn archive_keeps_the_cli_session_and_restore_resumes_it() {
    use crate::manager::session_dir::read_lineage;
    use brigadier_providers::model::Origin;
    let flow = Flow::start(
        "session-dir-lineage",
        Options {
            behavior: Arc::new(super::FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            ..Options::default()
        },
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Work.").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    let dir = flow.manager.session_dir(&id);
    let lineage = read_lineage(&dir.record());
    assert_eq!(lineage.len(), 1, "{lineage:?}");
    let native = lineage[0].session_id.clone();
    let file = std::path::PathBuf::from(lineage[0].file.clone().expect("the CLI's file"));
    assert!(file.exists(), "{}", file.display());

    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    super::eventually_async("the archive's cleanup to finish", || async {
        !flow.core.conversation(&id).unwrap().cleanup_pending
    })
    .await;
    assert!(file.exists(), "Archive keeps the CLI's session file");
    assert_eq!(read_lineage(&dir.record()), lineage, "and the lineage");

    flow.manager.restore(id.clone()).await.unwrap();
    flow.say("Carry on.").await;
    flow.settled().await;
    let specs = flow.thread_specs();
    let (_, spec) = specs.last().expect("the restored thread's CLI");
    assert_eq!(
        spec.origin,
        Origin::Resume {
            native_id: native.clone()
        }
    );
    assert_eq!(
        read_lineage(&dir.record()).len(),
        1,
        "a resumed session is listed once"
    );

    flow.manager.delete(id.clone()).await.unwrap();
    super::eventually("the session to go", || flow.core.conversation(&id).is_err()).await;
    flow.manager.cleanup_finished(&id).await;
    assert!(!file.exists(), "Delete removes the CLI's session file");
    assert!(!dir.root().exists(), "and the folder with its lineage");
    assert!(
        flow.manager
            .runtime
            .ledger()
            .artifacts(&crate::ledger::lineage_owner(&id.0))
            .is_empty()
    );
    flow.stop().await;
}
