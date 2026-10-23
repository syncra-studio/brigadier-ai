//! Settings › Git: the model that writes a blank commit message (Automatic is routing's pick
//! for git text; a usage limit hands it on; a chosen model that can't run falls back with a
//! note), and the commit and pull request styles in what the writer and the agents are told.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{Environment, GitStyle, ModelChoice, Settings, Setup};

/// Marks the message writer's sessions.
const WRITER: &str = "You write git commit messages";

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// One writer turn as its CLI saw it: its vendor and its instructions.
type Writes = Arc<Mutex<Vec<(ProviderKind, String)>>>;

/// A thread that only answers, and a writer that logs each turn and answers `reply` (by the
/// vendor it runs on).
fn writing(writes: &Writes, reply: fn(ProviderKind) -> Reply) -> Script {
    let writes = writes.clone();
    script(move |turn| {
        let writes = writes.clone();
        async move {
            if turn.prompt.contains(WRITER) {
                writes
                    .lock()
                    .unwrap()
                    .push((turn.provider, turn.prompt.clone()));
                return reply(turn.provider);
            }
            Reply::text("Hello.")
        }
    })
}

async fn edit_settings(flow: &Flow, edit: impl FnOnce(&mut Settings)) {
    let mut settings = flow.core.settings();
    edit(&mut settings);
    flow.core.update_settings(settings).await.unwrap();
}

/// Starts the session (its worktree) and leaves one change in it, staged.
async fn staged_change(flow: &Flow) -> PathBuf {
    flow.say("Hi.").await;
    flow.settled().await;
    let checkout = match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) => PathBuf::from(path),
        other => panic!("a session with a worktree, not {other:?}"),
    };
    std::fs::write(checkout.join("notes.txt"), "notes\n").unwrap();
    git(&checkout, &["add", "notes.txt"]);
    checkout
}

/// The writer's sessions, in order: vendor, model and effort.
fn writer_specs(flow: &Flow) -> Vec<(ProviderKind, Option<String>, Option<String>)> {
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| {
            spec.append_system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains(WRITER))
        })
        .map(|(provider, spec)| (provider, spec.model, spec.effort))
        .collect()
}

/// Whether each of the writer's sessions ran on its model's fast tier, in order.
fn writer_fast(flow: &Flow) -> Vec<bool> {
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| {
            spec.append_system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains(WRITER))
        })
        .map(|(_, spec)| spec.fast)
        .collect()
}

/// Automatic: routing's git text pick writes it (both fakes list one frontier model, so the
/// tie goes to Claude), in the repository's style, from its latest subjects.
#[tokio::test]
async fn automatic_writes_with_routings_pick_in_the_repositorys_style() {
    let writes = Writes::default();
    let flow = Flow::start(
        "git-writer-automatic",
        Options::default(),
        writing(&writes, |_| Reply::text("Add the notes")),
    )
    .await;
    staged_change(&flow).await;
    let outcome = flow
        .manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(outcome.message, "Add the notes");
    assert_eq!(outcome.note, None);
    let specs = writer_specs(&flow);
    assert_eq!(specs.len(), 1, "{specs:?}");
    assert_eq!(specs[0].0, ProviderKind::Claude);
    assert_eq!(specs[0].1.as_deref(), Some("claude-opus-5-5"));
    let prompt = writes.lock().unwrap()[0].1.clone();
    assert!(
        prompt.contains("Its latest commit subjects:\n- Start"),
        "{prompt}"
    );
    flow.stop().await;
}

/// A writer at its usage limit hands the message to routing's next pick: the other vendor.
#[tokio::test]
async fn a_writer_at_its_limit_hands_the_message_to_the_other_provider() {
    let writes = Writes::default();
    let flow = Flow::start(
        "git-writer-limit",
        Options::default(),
        writing(&writes, |provider| match provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Add the notes"),
        }),
    )
    .await;
    staged_change(&flow).await;
    let outcome = flow
        .manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(outcome.message, "Add the notes");
    let vendors: Vec<_> = writes.lock().unwrap().iter().map(|(p, _)| *p).collect();
    assert_eq!(vendors, [ProviderKind::Claude, ProviderKind::Codex]);
    flow.stop().await;
}

/// A chosen model writes it at its effort and speed; one that can't run falls back to Automatic, and
/// the commit says so.
#[tokio::test]
async fn a_chosen_writer_is_used_and_an_unavailable_one_falls_back_with_a_note() {
    let writes = Writes::default();
    let flow = Flow::start(
        "git-writer-chosen",
        Options::default(),
        writing(&writes, |_| Reply::text("Add the notes")),
    )
    .await;
    let checkout = staged_change(&flow).await;
    edit_settings(&flow, |settings| {
        settings.git_writer = Some(ModelChoice {
            provider: ProviderKind::Codex,
            model: Some("gpt-6-astra".into()),
            effort: Some("high".into()),
            fast: Some(true),
            account: None,
        });
    })
    .await;
    let outcome = flow
        .manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(outcome.note, None);
    assert_eq!(
        writer_specs(&flow).last().cloned(),
        Some((
            ProviderKind::Codex,
            Some("gpt-6-astra".into()),
            Some("high".into())
        ))
    );
    assert_eq!(writer_fast(&flow), [true], "on its fast tier, as chosen");

    // Made unavailable on the Providers page: Automatic writes it.
    std::fs::write(checkout.join("more.txt"), "more\n").unwrap();
    git(&checkout, &["add", "more.txt"]);
    edit_settings(&flow, |settings| {
        settings.hidden_models.push(crate::model::ModelRef {
            provider: ProviderKind::Codex,
            id: "gpt-6-astra".into(),
        });
    })
    .await;
    let outcome = flow
        .manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(
        outcome.note.as_deref(),
        Some(
            "Codex gpt-6-astra isn't available now, so Automatic wrote the message with Claude Code claude-opus-5-5."
        )
    );
    assert_eq!(
        writer_specs(&flow).last().map(|spec| spec.0),
        Some(ProviderKind::Claude)
    );
    assert_eq!(
        writer_fast(&flow),
        [true, false],
        "Automatic as routing sets it"
    );
    flow.stop().await;
}

/// Conventional Commits and the user's own instructions reach the writer; a message the
/// user typed is committed as typed, with no writer.
#[tokio::test]
async fn the_commit_style_reaches_the_writer() {
    let writes = Writes::default();
    let flow = Flow::start(
        "git-writer-style",
        Options::default(),
        writing(&writes, |_| Reply::text("feat: add the notes")),
    )
    .await;
    let checkout = staged_change(&flow).await;
    edit_settings(&flow, |settings| {
        settings.commit_style = GitStyle::Conventional;
    })
    .await;
    flow.manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    std::fs::write(checkout.join("more.txt"), "more\n").unwrap();
    git(&checkout, &["add", "more.txt"]);
    edit_settings(&flow, |settings| {
        settings.commit_style = GitStyle::Custom;
        settings.commit_instructions = "Start every subject with a ticket number.".into();
    })
    .await;
    flow.manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    std::fs::write(checkout.join("typed.txt"), "typed\n").unwrap();
    git(&checkout, &["add", "typed.txt"]);
    let typed = flow
        .manager
        .commit_changes(
            &flow.conversation,
            Some("Typed by me".into()),
            false,
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(typed.message, "Typed by me");
    let writes = writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 2, "no writer for a typed message");
    assert!(
        writes[0].1.contains("Conventional Commits 1.0.0"),
        "{}",
        writes[0].1
    );
    assert!(
        !writes[0].1.contains("latest commit subjects"),
        "{}",
        writes[0].1
    );
    assert!(
        writes[1]
            .1
            .contains("Follow the user's instructions:\nStart every subject with a ticket number."),
        "{}",
        writes[1].1
    );
    flow.stop().await;
}

/// Default styles add nothing to the thread's instructions: the same as before the setting.
#[tokio::test]
async fn default_styles_leave_the_instructions_as_they_were() {
    let flow = Flow::start(
        "git-writing-default",
        Options::default(),
        script(|_| async { Reply::text("Hello.") }),
    )
    .await;
    flow.say("Hi.").await;
    flow.settled().await;
    let prompt = flow.thread_specs()[0]
        .1
        .append_system_prompt
        .clone()
        .unwrap_or_default();
    assert!(!prompt.contains("Git writing"), "{prompt}");
    flow.stop().await;
}

/// The writer's turns as their CLIs saw them: vendor and account.
type Accounts = Arc<Mutex<Vec<(ProviderKind, Option<String>)>>>;

/// A thread that only answers, and a writer that logs vendor and account of each turn and
/// answers `reply` (by both).
fn writing_on(log: &Accounts, reply: fn(ProviderKind, Option<&str>) -> Reply) -> Script {
    let log = log.clone();
    script(move |turn| {
        let log = log.clone();
        async move {
            if turn.prompt.contains(WRITER) {
                log.lock()
                    .unwrap()
                    .push((turn.provider, turn.account.clone()));
                return reply(turn.provider, turn.account.as_deref());
            }
            Reply::text("Hello.")
        }
    })
}

/// A chosen writer naming an account at its limit moves to another account of the same
/// model, not to another model: the user's model still writes it, with no note.
#[tokio::test]
async fn a_chosen_writers_account_at_its_limit_hands_it_to_another_account() {
    let log = Accounts::default();
    let flow = Flow::start(
        "git-writer-account",
        Options::default(),
        writing_on(&log, |provider, account| match (provider, account) {
            (ProviderKind::Claude, None) => Reply::limited(),
            _ => Reply::text("Add the notes"),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    staged_change(&flow).await;
    edit_settings(&flow, |settings| {
        settings.git_writer = Some(ModelChoice {
            provider: ProviderKind::Claude,
            model: Some("claude-opus-5-5".into()),
            effort: None,
            fast: None,
            account: Some(crate::accounts::OWN.into()),
        });
    })
    .await;
    let outcome = flow
        .manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(outcome.message, "Add the notes");
    assert_eq!(outcome.note, None);
    assert_eq!(
        log.lock().unwrap().clone(),
        [
            (ProviderKind::Claude, None),
            (ProviderKind::Claude, Some("acct-b".into()))
        ]
    );
    assert!(
        writer_specs(&flow)
            .iter()
            .all(|spec| spec.1.as_deref() == Some("claude-opus-5-5")),
        "{:?}",
        writer_specs(&flow)
    );
    flow.stop().await;
}

/// A chosen writer at its limit with no other account: Automatic writes it, and the note
/// names the model that did, not the one that failed.
#[tokio::test]
async fn a_chosen_writer_at_its_limit_notes_the_model_that_wrote_it() {
    let log = Accounts::default();
    let flow = Flow::start(
        "git-writer-limit-note",
        Options::default(),
        writing_on(&log, |provider, _| match provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Add the notes"),
        }),
    )
    .await;
    staged_change(&flow).await;
    edit_settings(&flow, |settings| {
        settings.git_writer = Some(ModelChoice {
            provider: ProviderKind::Claude,
            model: Some("claude-opus-5-5".into()),
            effort: None,
            fast: None,
            account: None,
        });
    })
    .await;
    let outcome = flow
        .manager
        .commit_changes(&flow.conversation, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(outcome.message, "Add the notes");
    let last = writer_specs(&flow).last().cloned().unwrap();
    assert_eq!(last.0, ProviderKind::Codex);
    assert_eq!(
        outcome.note,
        Some(format!(
            "Claude Code claude-opus-5-5 isn't available now, so Automatic wrote the message with Codex {}.",
            last.1.unwrap()
        ))
    );
    flow.stop().await;
}

/// Before its session starts, a project's Files and Source panes act on the project's own
/// folder: its files, its changes staged, unstaged and discarded there, and a commit there with
/// a message written for it.
#[tokio::test]
async fn a_project_folder_lists_stages_and_commits_before_a_session_starts() {
    use crate::work::{SourceChange, SourceScope};

    let writes = Writes::default();
    let flow = Flow::start(
        "git-writer-project",
        Options::default(),
        writing(&writes, |_| Reply::text("Add the draft")),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .expect("a session has a project");
    let manager = &flow.manager;

    let (files, truncated) = manager.list_project_files(&project, None).await.unwrap();
    assert!(files.contains(&"README.md".to_owned()), "{files:?}");
    assert!(!truncated);

    std::fs::write(flow.repo.join("draft.txt"), "draft\n").unwrap();
    std::fs::write(flow.repo.join("README.md"), "# Changed\n").unwrap();
    let paths = |files: &[crate::work::SourceFile]| -> Vec<String> {
        files.iter().map(|file| file.path.clone()).collect()
    };
    let state = manager.project_source_state(&project).await.unwrap();
    assert_eq!(paths(&state.changes), ["README.md", "draft.txt"]);
    assert!(state.staged.is_empty());

    let only = |path: &str| Some(vec![path.to_owned()]);
    let state = manager
        .change_project_source(
            &project,
            SourceChange::Stage {
                paths: only("draft.txt"),
            },
        )
        .await
        .unwrap();
    assert_eq!(paths(&state.staged), ["draft.txt"]);
    let state = manager
        .change_project_source(&project, SourceChange::Unstage { paths: None })
        .await
        .unwrap();
    assert!(state.staged.is_empty());
    let state = manager
        .change_project_source(
            &project,
            SourceChange::Discard {
                scope: SourceScope::Unstaged,
                paths: only("README.md"),
            },
        )
        .await
        .unwrap();
    assert_eq!(paths(&state.changes), ["draft.txt"]);
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("README.md")).unwrap(),
        "# Flow\n"
    );

    manager
        .change_project_source(&project, SourceChange::Stage { paths: None })
        .await
        .unwrap();
    let outcome = manager
        .commit_project_changes(&project, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(outcome.message, "Add the draft");
    assert_eq!(outcome.branch, "main");
    assert_eq!(
        super::git(&flow.repo, &["log", "-1", "--format=%s"]).trim(),
        "Add the draft"
    );
    assert_eq!(writes.lock().unwrap().len(), 1);
    flow.stop().await;
}

/// A project's folder is acted on only through its own changes: a path out of the folder is
/// refused, a link to a file outside is discarded as the link (its target stays), and a project
/// that doesn't exist is refused before anything runs.
#[tokio::test]
async fn a_project_folder_stays_inside_itself() {
    use crate::model::ProjectId;
    use crate::work::{SourceChange, SourceScope};

    let flow = Flow::start(
        "git-writer-project-bounds",
        Options::default(),
        writing(&Writes::default(), |_| Reply::text("unused")),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .expect("a session has a project");
    let manager = &flow.manager;
    // Beside the repository, in the flow's own scratch folder.
    let secret = flow.repo.parent().unwrap().join("secret.txt");
    std::fs::write(&secret, "keep\n").unwrap();

    for path in ["../secret.txt", "/etc/hosts", ".git/config", "a/../../x"] {
        let refused = manager
            .change_project_source(
                &project,
                SourceChange::Discard {
                    scope: SourceScope::Unstaged,
                    paths: Some(vec![path.to_owned()]),
                },
            )
            .await;
        assert!(refused.is_err(), "{path} was accepted");
    }

    std::os::unix::fs::symlink(&secret, flow.repo.join("link.txt")).unwrap();
    manager
        .change_project_source(
            &project,
            SourceChange::Discard {
                scope: SourceScope::Unstaged,
                paths: Some(vec!["link.txt".to_owned()]),
            },
        )
        .await
        .unwrap();
    assert!(!flow.repo.join("link.txt").exists());
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "keep\n");

    let missing = ProjectId("no-such-project".into());
    assert!(manager.list_project_files(&missing, None).await.is_err());
    assert!(manager.project_source_state(&missing).await.is_err());
    assert!(
        manager
            .change_project_source(&missing, SourceChange::Stage { paths: None })
            .await
            .is_err()
    );
    assert!(
        manager
            .commit_project_changes(&missing, Some("x".into()), false, false, false)
            .await
            .is_err()
    );
    flow.stop().await;
}

/// The Source panel merges a branch in: the merge stops on its conflict, the conflicted file's
/// versions and sides come back, the user's resolution is written and staged, and the merge
/// commits with git's own message (no writer). A commit message is written into the box on
/// its own, and a session's panel lists its own checkout's branches.
#[tokio::test]
async fn the_source_panel_merges_resolves_and_commits_with_the_merge_message() {
    use crate::work::{SourceAction, SourceStatus, WorkspaceTarget};

    let writes = Writes::default();
    let flow = Flow::start(
        "git-writer-merge",
        Options::default(),
        writing(&writes, |_| Reply::text("Add the draft")),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .expect("a session has a project");
    let target = WorkspaceTarget::Project {
        project_id: project.clone(),
    };
    let manager = &flow.manager;
    let repo = &flow.repo;
    git(repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("README.md"), "# Feature\n").unwrap();
    git(repo, &["commit", "-qam", "Feature"]);
    git(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("README.md"), "# Main\n").unwrap();
    git(repo, &["commit", "-qam", "Main"]);

    let branches = manager.source_branches(&target).await.unwrap();
    let feature = branches
        .iter()
        .find(|b| b.name == "feature")
        .expect("feature");
    assert_eq!(feature.subject, "Feature");
    assert!(feature.at_ms > 1_000_000_000_000, "ms: {}", feature.at_ms);

    let state = manager
        .source_action(
            &target,
            SourceAction::Merge {
                branch: "feature".into(),
            },
        )
        .await
        .unwrap();
    let merging = state.merging.clone().expect("a merge in progress");
    assert_eq!(merging.incoming, "feature");
    assert!(
        state
            .changes
            .iter()
            .any(|f| f.path == "README.md" && f.status == SourceStatus::Conflicted)
    );
    let file = manager
        .conflict_file(&target, "README.md".into())
        .await
        .unwrap();
    assert!(
        file.current_label.ends_with(" • main"),
        "{}",
        file.current_label
    );
    assert!(
        file.incoming_label.ends_with(" • feature"),
        "{}",
        file.incoming_label
    );
    assert_eq!(file.incoming.as_deref(), Some("# Feature\n"));
    let text = file.text.expect("the marked file");
    let refused = manager
        .source_action(
            &target,
            SourceAction::Merge {
                branch: "feature".into(),
            },
        )
        .await;
    assert!(refused.is_err(), "one merge at a time");
    let state = manager
        .source_action(
            &target,
            SourceAction::Resolve {
                path: "README.md".into(),
                expected: Some(text),
                text: "# Both\n".into(),
                stage: true,
            },
        )
        .await
        .unwrap();
    assert!(state.changes.is_empty(), "{:?}", state.changes);
    let outcome = manager
        .commit_project_changes(&project, None, false, false, false)
        .await
        .unwrap();
    assert_eq!(outcome.message, merging.message);
    assert!(writes.lock().unwrap().is_empty(), "a merge needs no writer");
    assert_eq!(
        git(repo, &["rev-list", "--parents", "-n1", "HEAD"])
            .split_whitespace()
            .count(),
        3
    );
    let state = manager.project_source_state(&project).await.unwrap();
    assert!(state.merging.is_none());

    // A merge given up goes back to where it started.
    git(repo, &["checkout", "-q", "-b", "other", "HEAD~1"]);
    std::fs::write(repo.join("README.md"), "# Other\n").unwrap();
    git(repo, &["commit", "-qam", "Other"]);
    git(repo, &["checkout", "-q", "main"]);
    let state = manager
        .source_action(
            &target,
            SourceAction::Merge {
                branch: "other".into(),
            },
        )
        .await
        .unwrap();
    assert!(state.merging.is_some());
    let state = manager
        .source_action(&target, SourceAction::AbortMerge)
        .await
        .unwrap();
    assert!(state.merging.is_none() && state.changes.is_empty());
    // Git's refusal comes back in its own words.
    let error = manager
        .source_action(&target, SourceAction::Push)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("failed (Some("), "{error}");

    std::fs::write(repo.join("draft.txt"), "draft\n").unwrap();
    git(repo, &["add", "draft.txt"]);
    let (message, _) = manager.write_source_message(&target).await.unwrap();
    assert_eq!(message, "Add the draft");
    assert_eq!(
        git(repo, &["log", "-1", "--format=%s"]).trim(),
        "Merge branch 'feature'",
        "writing a message commits nothing"
    );

    // An amend with a blank message keeps the last commit's, AI co-author lines left out.
    git(
        repo,
        &[
            "commit",
            "-qm",
            "Draft\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
        ],
    );
    let outcome = manager
        .commit_project_changes(&project, None, false, false, true)
        .await
        .unwrap();
    assert_eq!(outcome.message.trim(), "Draft");
    assert_eq!(git(repo, &["log", "-1", "--format=%B"]).trim(), "Draft");
    assert_eq!(
        git(repo, &["log", "-1", "--format=%s", "HEAD~1"]).trim(),
        "Merge branch 'feature'",
        "amended in place"
    );

    staged_change(&flow).await;
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    let branches = manager.source_branches(&session).await.unwrap();
    assert!(branches.iter().any(|b| b.name == "main"), "{branches:?}");
    // A session's landings go to its own branch: it never switches away.
    let refused = manager
        .source_action(
            &session,
            SourceAction::Checkout {
                branch: "main".into(),
            },
        )
        .await;
    assert!(refused.is_err());
    flow.stop().await;
}
