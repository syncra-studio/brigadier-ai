//! S20's retention inventory: parent + worker, each with two CLI generations.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_brain::{
    NewNode, NodeKind, Origin, Provenance, RecordEntry, RecordFile, RecordSource,
};
use brigadier_cli::protocol::CliRequest;
use brigadier_providers::{Artifact, ProviderKind};

use super::{FakeBehavior, Flow, Options, Reply, Script, git};
use crate::manager::session_dir::{LineageEntry, note_lineage_at, read_lineage};
use crate::model::{ConversationId, Lifecycle};

struct OwnedProcess(std::process::Child);
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn script() -> Script {
    Arc::new(|turn| {
        Box::pin(async move {
            if turn.is_orchestrator() && turn.input.contains("Start worker") {
                let value = super::cli_tests::brigadier(
                    &turn,
                    CliRequest::Start(Box::new(super::cli_tests::start("Read the repository."))),
                )
                .await;
                assert!(value.get("error").is_none(), "{value}");
            }
            Reply::text("Done.")
        })
    })
}

fn inventory(root: &Path) -> Vec<String> {
    fn walk(root: &Path, path: &Path, out: &mut Vec<String>) {
        if !path.exists() {
            return;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

async fn retention(delete_directly: bool) {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w81-retention",
        Options {
            behavior: Arc::new(FakeBehavior {
                cleanup: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(),
    )
    .await;
    flow.say("Start worker").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    super::eventually_async("worker to report", || async {
        flow.core
            .tasks(&id)
            .await
            .unwrap()
            .first()
            .is_some_and(|t| t.state == crate::work::TaskState::Reported)
    })
    .await;
    let task = flow.core.tasks(&id).await.unwrap().remove(0);
    let dir = flow.manager.session_dir(&id);
    let worker = dir.worker(&task);
    let records = [dir.record(), worker.join("record")];
    let mut transcripts = Vec::new();
    let mut processes = Vec::new();
    // The first generation is the real fake CLI's own file. The second is a seam's file,
    // recorded exactly as the provider's cleanup artifacts + lineage register it.
    for (n, record) in records.iter().enumerate() {
        let first = read_lineage(record);
        assert_eq!(first.len(), 1, "{first:?}");
        transcripts.push(PathBuf::from(first[0].file.as_ref().unwrap()));
        let native_dir = flow.manager.data_dir.join(format!("w81-native-{n}"));
        std::fs::create_dir_all(&native_dir).unwrap();
        let file = native_dir.join("second.jsonl");
        std::fs::write(&file, "second generation").unwrap();
        transcripts.push(file.clone());
        flow.manager
            .runtime
            .ledger()
            .record(
                &crate::ledger::lineage_owner(&id.0),
                Artifact::ScratchDir {
                    path: native_dir.to_string_lossy().into_owned(),
                },
            )
            .await
            .unwrap();
        note_lineage_at(
            record.clone(),
            LineageEntry {
                provider: ProviderKind::Claude,
                session_id: format!("second-{n}"),
                file: Some(file.to_string_lossy().into_owned()),
                started_at: crate::now_ms(),
                forgotten: false,
            },
        )
        .await
        .unwrap();
        std::fs::write(record.join("ledger.md"), "decision + source").unwrap();
        for part in ["tmp", "dev", "logs"] {
            let folder = record.parent().unwrap().join(part);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("transient"), part).unwrap();
        }
        processes.push(OwnedProcess(
            std::process::Command::new("/bin/sleep")
                .arg("60")
                .current_dir(record.parent().unwrap().join("dev"))
                .spawn()
                .unwrap(),
        ));
    }
    let output_dir = PathBuf::from(&task.workspace.as_ref().unwrap().scratch).join("outputs");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::write(output_dir.join("deliverable.txt"), "worker deliverable").unwrap();
    // Give the worker the same kind of snapshot namespace as the parent's Changes cards.
    let worker_id = ConversationId(task.id.0.clone());
    let worker_refs = format!("refs/brigadier/s/{worker_id}/");
    git(
        &flow.repo,
        &["update-ref", &format!("{worker_refs}turn-1-start"), "HEAD"],
    );
    flow.manager
        .runtime
        .ledger()
        .record(
            &crate::manager::turn_changes::snapshots_owner(&worker_id),
            Artifact::GitRefs {
                repo: flow.repo.to_string_lossy().into_owned(),
                namespace: worker_refs.clone(),
            },
        )
        .await
        .unwrap();
    let refs = git(&flow.repo, &["for-each-ref", "refs/brigadier/s/"]);
    assert!(refs.contains(&format!("refs/brigadier/s/{id}/")) && refs.contains(&worker_refs));
    let parent_branch = match flow.core.conversation(&id).unwrap().setup.unwrap() {
        crate::model::Setup::Session {
            environment: crate::model::Environment::NewWorktree { branch, .. },
            ..
        } => branch,
        other => panic!("{other:?}"),
    };
    let worker_branch = task
        .workspace
        .as_ref()
        .unwrap()
        .branch
        .as_ref()
        .unwrap()
        .clone();
    let project = flow.core.conversation(&id).unwrap().project_id.unwrap();
    let brain = flow
        .manager
        .project_brain(&project)
        .await
        .unwrap()
        .brain
        .clone();
    let sessions = [id.0.clone(), task.id.0.clone(), "unrelated".into()];
    let node_ids = tokio::task::spawn_blocking({
        let (brain, sessions) = (brain.clone(), sessions.clone());
        move || {
            sessions
                .iter()
                .map(|session| {
                    brain
                        .sync_records(
                            RecordFile {
                                conversation_id: session.clone(),
                                session: session.clone(),
                                source: RecordSource::Ledger,
                                path: format!("/r/{session}/ledger.md"),
                                stamp: "1".into(),
                            },
                            vec![RecordEntry {
                                item_id: None,
                                tool: None,
                                position: 1,
                                role: "ledger".into(),
                                at_ms: Some(crate::now_ms()),
                                text: "retention evidence".into(),
                                cited: None,
                            }],
                        )
                        .unwrap();
                    brain
                        .record(NewNode {
                            kind: NodeKind::Decision,
                            key: None,
                            title: format!("Decision {session}"),
                            body: format!("Evidence belonging to {session}"),
                            provenance: Provenance {
                                origin: Origin::User,
                                session_id: Some(session.clone()),
                                task_id: None,
                                job_id: None,
                                worker: None,
                                commit: None,
                                recorded_at_ms: crate::now_ms(),
                                claim: None,
                            },
                            files: Vec::new(),
                            expires_at_ms: None,
                        })
                        .unwrap()
                })
                .collect::<Vec<_>>()
        }
    })
    .await
    .unwrap();
    let record_before: Vec<_> = records.iter().map(|r| inventory(r)).collect();
    if !delete_directly {
        flow.manager.archive(id.clone()).await.unwrap();
        flow.manager.cleanup_finished(&id).await;
        assert_eq!(
            flow.core.conversation(&id).unwrap().lifecycle,
            Lifecycle::Archived
        );
        for process in &mut processes {
            assert!(
                process.0.try_wait().unwrap().is_some(),
                "Archive stopped both owners' detached processes"
            );
        }
        // Exact leaves: record files, two transcripts each, both snapshot namespaces, all
        // session Brain nodes/indexes; neither worktree nor tmp/dev/logs for either owner.
        assert_eq!(
            records.iter().map(|r| inventory(r)).collect::<Vec<_>>(),
            record_before
        );
        for folder in [dir.root(), worker.as_path()] {
            for part in ["work", "tmp", "dev", "logs"] {
                assert!(
                    !folder.join(part).exists(),
                    "{}",
                    folder.join(part).display()
                );
            }
        }
        assert!(
            transcripts.iter().all(|path| path.is_file()),
            "{transcripts:?}"
        );
        assert_eq!(
            git(&flow.repo, &["for-each-ref", "refs/brigadier/s/"]),
            refs
        );
        for node in &node_ids {
            assert!(brain.node(node).unwrap().is_some());
        }
        for session in &sessions {
            assert_eq!(
                brain
                    .record_stamp(&format!("/r/{session}/ledger.md"))
                    .unwrap()
                    .as_deref(),
                Some("1")
            );
        }
        let saved = flow.core.tasks(&id).await.unwrap().remove(0).outputs;
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].file_name.as_deref(), Some("deliverable.txt"));
        assert_eq!(
            std::fs::read(
                flow.core
                    .store()
                    .blobs()
                    .file_of(&saved[0].id.parse().unwrap())
            )
            .unwrap(),
            b"worker deliverable"
        );
        assert!(
            flow.core.tasks(&id).await.unwrap()[0]
                .plain
                .as_ref()
                .unwrap()
                .closed
        );
        let (items, _) = flow
            .manager
            .scan_storage(crate::manager::disk::ScanContext {
                agent_homes: Some(Vec::new()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!items.iter().any(|row| matches!(&row.action, crate::manager::disk::Action::Dispose { owners } if owners.iter().any(|(owner, _)| owner == &crate::manager::turn_changes::snapshots_owner(&worker_id)))), "Storage retains archived workers' snapshot refs");
        flow.manager.restore(id.clone()).await.unwrap();
        assert_eq!(
            flow.core.conversation(&id).unwrap().lifecycle,
            Lifecycle::Active
        );
        assert_eq!(
            records.iter().map(|r| inventory(r)).collect::<Vec<_>>(),
            record_before
        );
    }
    flow.manager.delete(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(flow.core.conversation(&id).is_err());
    for process in &mut processes {
        assert!(
            process.0.try_wait().unwrap().is_some(),
            "Delete stopped both owners' detached processes"
        );
    }
    assert!(!dir.root().exists());
    assert!(
        transcripts.iter().all(|path| !path.exists()),
        "{transcripts:?}"
    );
    assert!(git(&flow.repo, &["for-each-ref", "refs/brigadier/s/"]).is_empty());
    for branch in [&parent_branch, &worker_branch] {
        assert!(
            git(&flow.repo, &["branch", "--list", branch]).is_empty(),
            "Delete removes {branch}"
        );
    }
    for node in &node_ids[..2] {
        assert!(brain.node(node).unwrap().is_none());
    }
    assert!(brain.node(&node_ids[2]).unwrap().is_some());
    for session in &sessions[..2] {
        assert_eq!(
            brain
                .record_stamp(&format!("/r/{session}/ledger.md"))
                .unwrap(),
            None
        );
    }
    assert_eq!(
        brain
            .record_stamp(&format!("/r/{}/ledger.md", sessions[2]))
            .unwrap()
            .as_deref(),
        Some("1")
    );
    flow.stop().await;
}

#[tokio::test]
async fn archive_restore_delete_retention_inventory_includes_workers() {
    retention(false).await;
}

#[tokio::test]
async fn direct_delete_retention_inventory_includes_workers() {
    retention(true).await;
}

#[tokio::test]
async fn archive_keeps_unmerged_and_dirty_parent_and_worker_worktrees() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w81-unmerged",
        Options::default(),
        script(),
    )
    .await;
    flow.say("Start worker").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    super::eventually_async("worker to report", || async {
        flow.core
            .tasks(&id)
            .await
            .unwrap()
            .first()
            .is_some_and(|t| t.state == crate::work::TaskState::Reported)
    })
    .await;
    let task = flow.core.tasks(&id).await.unwrap().remove(0);
    let parent = flow.manager.session_dir(&id).work();
    let worker = flow.manager.session_dir(&id).worker(&task).join("work");
    std::fs::write(flow.repo.join(".git/info/exclude"), ".next/\n").unwrap();
    let mut heads = Vec::new();
    for (n, path) in [&parent, &worker].into_iter().enumerate() {
        let file = format!("committed-{n}");
        std::fs::write(path.join(&file), "unmerged work").unwrap();
        git(path, &["add", &file]);
        git(path, &["commit", "-m", "unmerged"]);
        heads.push(git(path, &["rev-parse", "HEAD"]));
        std::fs::write(path.join("dirty"), "unsaved work").unwrap();
        std::fs::create_dir_all(path.join(".next")).unwrap();
        std::fs::write(path.join(".next/build"), "output").unwrap();
    }
    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    for (path, head) in [&parent, &worker].into_iter().zip(&heads) {
        assert!(path.join(".git").is_file(), "unmerged worktree stays");
        assert_eq!(
            std::fs::read_to_string(path.join("dirty")).unwrap(),
            "unsaved work"
        );
        assert_eq!(
            &git(path, &["rev-parse", "HEAD"]),
            head,
            "Archive creates no WIP commit"
        );
        assert!(
            !path.join(".next").exists(),
            "build output goes even in retained worktrees"
        );
    }
    let (items, _) = flow
        .manager
        .scan_storage(crate::manager::disk::ScanContext {
            agent_homes: Some(Vec::new()),
            ..Default::default()
        })
        .await
        .unwrap();
    let advice: Vec<_> = items
        .iter()
        .filter(|row| row.item.label.starts_with("Worktree cleanup"))
        .collect();
    assert_eq!(advice.len(), 2);
    for row in advice {
        assert!(
            !row.item.selectable && !row.item.checked,
            "dirty work needs the existing ask"
        );
        assert!(row.item.reason.contains("not merged confirmation"));
    }
    flow.manager.restore(id.clone()).await.unwrap();
    assert!(parent.join("dirty").is_file() && worker.join("dirty").is_file());
    flow.manager.delete(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(!flow.manager.session_dir(&id).root().exists());
    flow.stop().await;
}

#[tokio::test]
async fn archive_retries_a_failed_merged_worker_worktree_removal() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w81-retry",
        Options::default(),
        script(),
    )
    .await;
    flow.say("Start worker").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    super::eventually_async("worker to report", || async {
        flow.core
            .tasks(&id)
            .await
            .unwrap()
            .first()
            .is_some_and(|t| t.state == crate::work::TaskState::Reported)
    })
    .await;
    let task = flow.core.tasks(&id).await.unwrap().remove(0);
    let work = flow.manager.session_dir(&id).worker(&task).join("work");
    let held = work.clone();
    flow.manager
        .runtime
        .ledger()
        .set_worktree_remover(Arc::new(move |repo, path| {
            let fail = path == held;
            Box::pin(async move {
                if fail {
                    return Err("temporary removal failure".into());
                }
                git(
                    &repo,
                    &["worktree", "remove", "--force", &path.to_string_lossy()],
                );
                Ok(())
            })
        }));
    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(work.exists());
    assert!(
        flow.manager
            .runtime
            .ledger()
            .disposing()
            .contains(&format!("task:{}", task.id))
    );
    flow.manager.install_worktree_remover();
    flow.manager.runtime.ledger().sweep().await;
    assert!(
        !work.exists(),
        "the durable disposal retries at the next sweep"
    );
    assert!(
        flow.manager
            .session_dir(&id)
            .worker(&task)
            .join("record/workspace.json")
            .exists()
    );
    flow.stop().await;
}

#[tokio::test]
async fn archive_never_removes_build_output_through_a_replaced_worktree_link() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w81-link",
        Options::default(),
        script(),
    )
    .await;
    flow.say("Look around").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    let work = flow.manager.session_dir(&id).work();
    let outside = flow.manager.data_dir.join("w81-other-checkout");
    std::fs::write(flow.repo.join(".git/info/exclude"), ".next/\n").unwrap();
    std::fs::create_dir_all(work.join(".next")).unwrap();
    std::fs::write(work.join(".next/output"), "keep external output").unwrap();
    std::fs::rename(&work, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &work).unwrap();
    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(outside.join(".next/output").exists());
    std::fs::remove_file(&work).unwrap();
    std::fs::rename(&outside, &work).unwrap();
    flow.stop().await;
}

/// #563: a worker that had reported reads so after Archive and Restore. Kept with its
/// worktree, its session can still send it more; one whose worktree went with the archive
/// (merged and clean) is closed.
#[tokio::test]
async fn a_reported_worker_reads_reported_after_archive_and_restore() {
    for unmerged in [true, false] {
        let flow = Flow::start(
            &format!("20261022-0805-take-over-5-sessions-w139-restore-{unmerged}"),
            Options::default(),
            script(),
        )
        .await;
        flow.say("Start worker").await;
        flow.settled().await;
        let id = flow.conversation.clone();
        super::eventually_async("worker to report", || async {
            flow.core
                .tasks(&id)
                .await
                .unwrap()
                .first()
                .is_some_and(|t| t.state == crate::work::TaskState::Reported)
        })
        .await;
        let task = flow.core.tasks(&id).await.unwrap().remove(0);
        let worker = flow.manager.session_dir(&id).worker(&task).join("work");
        if unmerged {
            std::fs::write(worker.join("work.txt"), "unmerged work").unwrap();
            git(&worker, &["add", "work.txt"]);
            git(&worker, &["commit", "-m", "unmerged"]);
        }
        flow.manager.archive(id.clone()).await.unwrap();
        flow.manager.cleanup_finished(&id).await;
        assert_eq!(worker.exists(), unmerged);
        flow.manager.restore(id.clone()).await.unwrap();
        let task = flow.core.tasks(&id).await.unwrap().remove(0);
        assert_eq!(task.state, crate::work::TaskState::Reported, "{unmerged}");
        assert_eq!(
            task.plain.as_ref().unwrap().closed,
            !unmerged,
            "`brigadier workers` lists it while its worktree is kept"
        );
        flow.stop().await;
    }
}
