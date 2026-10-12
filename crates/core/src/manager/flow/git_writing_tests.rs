//! Settings › Git: the model that writes a blank commit message (Automatic is routing's pick
//! for git text; a usage limit hands it on; a chosen model that can't run falls back with a
//! note), and the commit and pull request styles in what the writer and the agents are told.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use serde_json::json;

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
        .commit_changes(&flow.conversation, None, false, false)
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
        .commit_changes(&flow.conversation, None, false, false)
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
        .commit_changes(&flow.conversation, None, false, false)
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
        .commit_changes(&flow.conversation, None, false, false)
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
        .commit_changes(&flow.conversation, None, false, false)
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
        .commit_changes(&flow.conversation, None, false, false)
        .await
        .unwrap();
    std::fs::write(checkout.join("typed.txt"), "typed\n").unwrap();
    git(&checkout, &["add", "typed.txt"]);
    let typed = flow
        .manager
        .commit_changes(&flow.conversation, Some("Typed by me".into()), false, false)
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

/// The thread starts with the user's styles in its instructions; switched while it runs, it
/// hears the new ones once, before its next turn. Its lead's brief carries them too.
#[tokio::test]
async fn the_styles_reach_the_thread_and_its_workers_and_a_switch_reaches_a_running_thread() {
    let inputs = Arc::new(Mutex::new(Vec::<String>::new()));
    let briefs = Arc::new(Mutex::new(Vec::<String>::new()));
    let (seen, told) = (inputs.clone(), briefs.clone());
    let flow = Flow::start(
        "git-writing-agents",
        Options::default(),
        script(move |turn| {
            let (seen, told) = (seen.clone(), told.clone());
            async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("Add a greeting.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Add a greeting",
                                       "kind": "implement", "spec": "Create hello.txt.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.task_number().is_some() && turn.earlier == 0 {
                    told.lock().unwrap().push(turn.input.clone());
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Nothing to change.", "changes": []}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    edit_settings(&flow, |settings| {
        settings.commit_style = GitStyle::Custom;
        settings.commit_instructions = "Prefix subjects with the area.".into();
        settings.pr_style = GitStyle::Conventional;
    })
    .await;
    flow.say("Hi.").await;
    flow.settled().await;
    let threads = flow.thread_specs();
    let prompt = threads[0]
        .1
        .append_system_prompt
        .clone()
        .unwrap_or_default();
    assert!(
        prompt.contains("Git writing (the user's settings"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Prefix subjects with the area."),
        "{prompt}"
    );
    assert!(
        prompt.contains("Give the title the Conventional Commits form"),
        "{prompt}"
    );

    flow.say("Add a greeting.").await;
    flow.settled().await;
    let briefs = briefs.lock().unwrap().clone();
    assert_eq!(briefs.len(), 1, "{briefs:#?}");
    assert!(
        briefs[0].contains("Prefix subjects with the area."),
        "{}",
        briefs[0]
    );

    // Back to the repository's way: the running thread hears it once.
    edit_settings(&flow, |settings| {
        settings.commit_style = GitStyle::Repository;
        settings.pr_style = GitStyle::Repository;
    })
    .await;
    flow.say("And now?").await;
    flow.settled().await;
    flow.say("Once more.").await;
    flow.settled().await;
    let inputs = inputs.lock().unwrap().clone();
    let heard: Vec<_> = inputs
        .iter()
        .filter(|input| input.contains("[settings] The user changed how commit messages"))
        .collect();
    assert_eq!(heard.len(), 1, "{inputs:#?}");
    assert!(heard[0].contains("And now?"), "{}", heard[0]);
    assert!(
        heard[0].contains("follow this repository's conventions for both"),
        "{}",
        heard[0]
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

/// A worker that builds hears a switch at once while it works, and only once.
#[tokio::test]
async fn a_working_worker_hears_a_switch_at_once() {
    let steered = Arc::new(Mutex::new(Vec::<String>::new()));
    let heard = steered.clone();
    let flow = Flow::start(
        "git-writing-worker",
        Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Add a greeting.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Add a greeting",
                                       "kind": "implement", "spec": "Create hello.txt.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.task_number().is_some() && turn.earlier == 0 {
                    assert!(!turn.input.contains("Git writing"), "{}", turn.input);
                    let note = turn.steered().await.expect("told of the switch");
                    heard.lock().unwrap().push(note);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Nothing to change.", "changes": []}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add a greeting.").await;
    flow.until("the lead to work", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == crate::work::TaskState::Running)
    })
    .await;
    // Its turn has started: what it was told is its brief.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    edit_settings(&flow, |settings| {
        settings.commit_style = GitStyle::Conventional;
    })
    .await;
    flow.manager.git_writing_switched().await;
    // A second call tells it nothing new.
    flow.manager.git_writing_switched().await;
    flow.settled().await;
    let notes = steered.lock().unwrap().clone();
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert!(
        notes[0].contains("[settings] The user changed how commit messages"),
        "{}",
        notes[0]
    );
    assert!(
        notes[0].contains("Conventional Commits 1.0.0"),
        "{}",
        notes[0]
    );
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
        .commit_changes(&flow.conversation, None, false, false)
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
        .commit_changes(&flow.conversation, None, false, false)
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
        .commit_project_changes(&project, None, false, false)
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
