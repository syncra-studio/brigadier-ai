use super::*;
use crate::manager::flow::{Flow, Options, Reply};
use brigadier_providers::ProviderKind;
use std::path::Path;
use std::sync::Arc;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill_text(name: &str, body: &str) -> String {
    format!("---\nname: {name}\n---\n{body}\n")
}

async fn fixture(name: &str) -> Flow {
    Flow::start(
        name,
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await
}

/// `name`'s skill in Claude's own folder.
fn claude_skill(flow: &Flow, name: &str, body: &str) -> std::path::PathBuf {
    let dir = flow
        .manager
        .registry()
        .roots()
        .claude_dir
        .join("skills")
        .join(name);
    write(&dir.join("SKILL.md"), &skill_text(name, body));
    dir
}

fn row_on(flow: &Flow, name: &str) -> Option<bool> {
    flow.manager
        .plugin_catalog()
        .unwrap()
        .rows
        .iter()
        .find(|row| row.name == name)
        .map(|row| row.choice.on)
}

async fn changes(flow: &Flow) -> PluginChanges {
    flow.manager.check_plugin_changes().await.unwrap()
}

fn names(changes: &PluginChanges) -> (Vec<&str>, Vec<&str>) {
    (
        changes
            .found
            .iter()
            .map(|item| item.name.as_str())
            .collect(),
        changes.gone.iter().map(|item| item.name.as_str()).collect(),
    )
}

async fn apply(flow: &Flow, add: &[&String], decline: &[&String], remove: &[&str], keep: &[&str]) {
    let owned = |keys: &[&String]| keys.iter().map(|key| (*key).clone()).collect::<Vec<_>>();
    let plain = |keys: &[&str]| keys.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>();
    flow.manager
        .apply_plugin_changes(owned(add), owned(decline), plain(remove), plain(keep))
        .await
        .unwrap();
}

/// A new session's setup for `provider`, as its launch resolved it.
async fn launch(flow: &Flow, provider: ProviderKind) -> brigadier_providers::SessionSpec {
    let before = flow.specs().len();
    let session = flow.add_session("Launch", None, provider).await;
    flow.say_to(&session, "Check launch.").await;
    flow.idle_in(&session).await;
    flow.specs()[before..]
        .iter()
        .find(|(kind, _)| *kind == provider)
        .unwrap()
        .1
        .clone()
}

#[tokio::test]
async fn the_first_read_accepts_everything_already_there() {
    let flow = fixture("known-baseline").await;
    claude_skill(&flow, "here-first", "one");
    let roots = flow.manager.registry().roots().clone();
    write(
        &roots.claude_json,
        r#"{"mcpServers":{"here-too":{"command":"/app/here"}}}"#,
    );
    assert!(!flow.manager.data_dir.join(FILE).exists());
    assert_eq!(row_on(&flow, "here-first"), Some(true));
    assert_eq!(row_on(&flow, "here-too"), Some(true));
    assert!(flow.manager.data_dir.join(FILE).exists());
    assert_eq!(changes(&flow).await, PluginChanges::default());
    // The list accepts them without saving a choice, so their derived scope stays derived.
    assert!(Choices::load(&flow.manager.data_dir).unwrap().0.is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn a_new_item_is_off_and_out_of_launches_until_it_is_added() {
    let flow = fixture("known-new-off").await;
    flow.manager.plugin_catalog().unwrap();
    let roots = flow.manager.registry().roots().clone();
    claude_skill(&flow, "fresh-skill", "one");
    write(
        &roots.claude_json,
        r#"{"mcpServers":{"fresh-server":{"command":"/app/fresh"}}}"#,
    );
    assert_eq!(row_on(&flow, "fresh-skill"), Some(false));
    assert_eq!(row_on(&flow, "fresh-server"), Some(false));
    let spec = launch(&flow, ProviderKind::Claude).await;
    assert!(spec.mcp_servers.iter().all(|s| s.name != "fresh-server"));
    assert!(
        spec.switched_off
            .servers
            .iter()
            .any(|name| name == "fresh-server")
    );
    assert!(
        spec.switched_off
            .skill_names
            .iter()
            .any(|name| name == "fresh-skill")
    );

    let found = changes(&flow).await;
    assert_eq!(names(&found).0, ["fresh-server", "fresh-skill"]);
    assert_eq!(found.found[0].detail, "/app/fresh");
    assert_eq!(found.found[1].agents, [ProviderKind::Claude]);
    let keys: Vec<_> = found.found.iter().map(|item| &item.key).collect();
    apply(&flow, &keys, &[], &[], &[]).await;

    assert_eq!(changes(&flow).await, PluginChanges::default());
    assert_eq!(row_on(&flow, "fresh-skill"), Some(true));
    assert_eq!(row_on(&flow, "fresh-server"), Some(true));
    let spec = launch(&flow, ProviderKind::Claude).await;
    assert!(spec.mcp_servers.iter().any(|s| s.name == "fresh-server"));
    assert!(
        !spec
            .switched_off
            .skill_names
            .iter()
            .any(|name| name == "fresh-skill")
    );
    flow.stop().await;
}

#[tokio::test]
async fn switching_a_new_item_on_by_hand_counts_as_choosing_it() {
    let flow = fixture("known-by-hand").await;
    flow.manager.plugin_catalog().unwrap();
    claude_skill(&flow, "by-hand", "one");
    let found = changes(&flow).await;
    flow.manager
        .set_plugin_choice(&found.found[0].key, true)
        .unwrap();
    assert_eq!(row_on(&flow, "by-hand"), Some(true));
    assert_eq!(changes(&flow).await, PluginChanges::default());
    flow.stop().await;
}

#[tokio::test]
async fn a_declined_item_stays_hidden_through_an_edit_and_add_brings_it_back() {
    let flow = fixture("known-decline").await;
    flow.manager.plugin_catalog().unwrap();
    let dir = claude_skill(&flow, "declined", "one");
    let before = std::fs::read(dir.join("SKILL.md")).unwrap();
    let found = changes(&flow).await;
    apply(&flow, &[], &[&found.found[0].key], &[], &[]).await;

    assert_eq!(std::fs::read(dir.join("SKILL.md")).unwrap(), before);
    assert_eq!(changes(&flow).await, PluginChanges::default());
    let view = flow.manager.plugin_catalog().unwrap();
    assert!(view.rows.iter().all(|row| row.name != "declined"));
    assert_eq!(view.hidden.as_ref().unwrap()[0].name, "declined");
    let spec = launch(&flow, ProviderKind::Claude).await;
    assert!(
        spec.switched_off
            .skill_names
            .iter()
            .any(|name| name == "declined")
    );

    // An update gives it a new id; it's still the item the user said no to.
    claude_skill(&flow, "declined", "two");
    assert_eq!(changes(&flow).await, PluginChanges::default());
    let hidden = flow.manager.plugin_catalog().unwrap().hidden.unwrap();
    assert_eq!(hidden[0].name, "declined");
    assert_ne!(hidden[0].key, found.found[0].key);

    flow.manager
        .restore_plugin(hidden[0].key.clone())
        .await
        .unwrap();
    assert_eq!(row_on(&flow, "declined"), Some(true));
    assert!(flow.manager.plugin_catalog().unwrap().hidden.is_none());
    assert_eq!(changes(&flow).await, PluginChanges::default());
    flow.stop().await;
}

#[tokio::test]
async fn removing_a_gone_item_clears_its_settings_and_its_return_is_new() {
    let flow = fixture("known-gone-remove").await;
    let dir = claude_skill(&flow, "going", "one");
    let id = flow.manager.plugin_catalog().unwrap().rows[0].id.clone();
    flow.manager.set_plugin_choice(&id, false).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    let found = changes(&flow).await;
    assert_eq!(names(&found).1, ["going"]);
    apply(&flow, &[], &[], &["skill:going"], &[]).await;

    assert_eq!(changes(&flow).await, PluginChanges::default());
    assert!(flow.manager.plugin_catalog().unwrap().missing.is_none());
    assert!(
        !Choices::load(&flow.manager.data_dir)
            .unwrap()
            .0
            .contains_key(&id)
    );
    claude_skill(&flow, "going", "one");
    assert_eq!(names(&changes(&flow).await).0, ["going"]);
    flow.stop().await;
}

#[tokio::test]
async fn a_kept_item_is_listed_missing_and_returns_with_its_settings() {
    let flow = fixture("known-gone-keep").await;
    let dir = claude_skill(&flow, "kept", "one");
    let id = flow.manager.plugin_catalog().unwrap().rows[0].id.clone();
    flow.manager.set_plugin_choice(&id, false).unwrap();
    let scope = Scope::Projects(vec![flow.repo.clone()]);
    flow.manager
        .set_plugin_choice_scope(&id, scope.clone())
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    apply(&flow, &[], &[], &[], &["skill:kept"]).await;

    assert_eq!(changes(&flow).await, PluginChanges::default());
    let missing = flow.manager.plugin_catalog().unwrap().missing.unwrap();
    assert_eq!(missing[0].name, "kept");
    assert_eq!(missing[0].kind, PluginKind::Skill);

    // Its files come back changed: a new id, the same item, the same settings.
    claude_skill(&flow, "kept", "two");
    let view = flow.manager.plugin_catalog().unwrap();
    assert!(view.missing.is_none());
    let row = view.rows.iter().find(|row| row.name == "kept").unwrap();
    assert_ne!(row.id, id);
    assert!(!row.choice.on);
    assert_eq!(row.choice.scope, scope);
    assert_eq!(changes(&flow).await, PluginChanges::default());
    flow.stop().await;
}

#[tokio::test]
async fn an_item_still_in_another_cli_is_not_gone() {
    let flow = fixture("known-other-cli").await;
    let roots = flow.manager.registry().roots().clone();
    let claude = claude_skill(&flow, "shared", "one");
    write(
        &roots.codex_home.join("skills/shared/SKILL.md"),
        &skill_text("shared", "one"),
    );
    flow.manager.plugin_catalog().unwrap();
    std::fs::remove_dir_all(&claude).unwrap();
    assert_eq!(changes(&flow).await, PluginChanges::default());
    assert_eq!(row_on(&flow, "shared"), Some(true));
    flow.stop().await;
}

#[tokio::test]
async fn an_edit_is_not_new_or_gone_and_keeps_its_settings() {
    let flow = fixture("known-edit").await;
    claude_skill(&flow, "edited", "one");
    let id = flow.manager.plugin_catalog().unwrap().rows[0].id.clone();
    flow.manager.set_plugin_choice(&id, false).unwrap();
    claude_skill(&flow, "edited", "two");
    assert_eq!(changes(&flow).await, PluginChanges::default());
    let view = flow.manager.plugin_catalog().unwrap();
    assert_ne!(view.rows[0].id, id);
    assert!(!view.rows[0].choice.on);
    // A later change saves under the new id, and the earlier one goes.
    flow.manager
        .set_plugin_choice(&view.rows[0].id, true)
        .unwrap();
    let saved = Choices::load(&flow.manager.data_dir).unwrap();
    assert!(!saved.0.contains_key(&id));
    assert!(saved.0[&view.rows[0].id].on);
    flow.stop().await;
}

#[tokio::test]
async fn project_only_items_and_import_offers_are_never_asked_about() {
    let flow = fixture("known-exempt").await;
    flow.manager.plugin_catalog().unwrap();
    let roots = flow.manager.registry().roots().clone();
    write(
        &flow.repo.join(".mcp.json"),
        r#"{"mcpServers":{"own-server":{"command":"/app/own"}}}"#,
    );
    write(
        &flow.repo.join(".claude/skills/own-skill/SKILL.md"),
        &skill_text("own-skill", "one"),
    );
    write(
        &roots.cursor_dir.join("mcp.json"),
        r#"{"mcpServers":{"offer":{"command":"/app/offer"}}}"#,
    );
    assert_eq!(changes(&flow).await, PluginChanges::default());
    let view = flow.manager.plugin_catalog().unwrap();
    for name in ["own-server", "own-skill"] {
        let row = view.rows.iter().find(|row| row.name == name).unwrap();
        assert!(row.choice.on, "{name} stays on");
        assert_eq!(row.choice.scope, Scope::Projects(vec![flow.repo.clone()]));
    }
    // A project's item leaving, with its branch, isn't asked about either.
    std::fs::remove_file(flow.repo.join(".mcp.json")).unwrap();
    assert_eq!(changes(&flow).await, PluginChanges::default());
    flow.stop().await;
}

#[tokio::test]
async fn what_add_adds_is_accepted_and_what_remove_removes_is_not_gone() {
    let flow = fixture("known-library").await;
    flow.manager.plugin_catalog().unwrap();
    flow.manager
        .add_library_item(
            "https://library.test/mcp".into(),
            Some("added".into()),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(row_on(&flow, "added"), Some(true));
    assert_eq!(changes(&flow).await, PluginChanges::default());
    flow.manager
        .remove_library_item("lib:added".into())
        .await
        .unwrap();
    assert_eq!(changes(&flow).await, PluginChanges::default());
    flow.stop().await;
}

#[tokio::test]
async fn a_declined_item_moved_into_a_project_stays_hidden_after_an_edit() {
    let flow = fixture("known-decline-moved").await;
    flow.manager.plugin_catalog().unwrap();
    let dir = claude_skill(&flow, "moved", "user version");
    let found = changes(&flow).await;
    apply(&flow, &[], &[&found.found[0].key], &[], &[]).await;
    std::fs::remove_dir_all(dir).unwrap();
    write(
        &flow.repo.join(".claude/skills/moved/SKILL.md"),
        &skill_text("moved", "project version"),
    );

    assert_eq!(changes(&flow).await, PluginChanges::default());
    let view = flow.manager.plugin_catalog().unwrap();
    assert!(view.rows.iter().all(|row| row.name != "moved"));
    let hidden = view.hidden.unwrap();
    assert_eq!(hidden[0].name, "moved");
    assert_ne!(hidden[0].key, found.found[0].key);
    // Resolve the checkout containing the new file; a fresh session's worktree would not
    // contain this uncommitted fixture and would have no native skill to suppress.
    let resolved = flow
        .manager
        .resolve_plugins(
            &flow
                .manager
                .runtime
                .launch_account(ProviderKind::Claude, None),
            Some(&flow.repo),
            &flow.repo,
            true,
            super::super::resolve::Role::Thread,
        )
        .await
        .unwrap();
    assert!(resolved.skills.iter().all(|skill| skill.display != "moved"));
    assert!(resolved.off.skill_names.iter().any(|name| name == "moved"));

    flow.manager
        .restore_plugin(hidden[0].key.clone())
        .await
        .unwrap();
    assert_eq!(row_on(&flow, "moved"), Some(true));
    flow.stop().await;
}

#[tokio::test]
async fn saved_project_settings_follow_an_edited_row_without_enrolling_it_in_refresh() {
    let flow = fixture("known-project-edit").await;
    flow.manager.plugin_catalog().unwrap();
    let file = flow.repo.join(".claude/skills/local/SKILL.md");
    write(&file, &skill_text("local", "one"));
    let view = flow.manager.plugin_catalog().unwrap();
    let row = view.rows.iter().find(|row| row.name == "local").unwrap();
    let old_id = row.id.clone();
    flow.manager.set_plugin_choice(&old_id, false).unwrap();
    write(&file, &skill_text("local", "two"));
    let view = flow.manager.plugin_catalog().unwrap();
    let row = view.rows.iter().find(|row| row.name == "local").unwrap();
    assert_ne!(row.id, old_id);
    assert!(!row.choice.on);
    assert_eq!(row.choice.scope, Scope::Projects(vec![flow.repo.clone()]));
    assert_eq!(changes(&flow).await, PluginChanges::default());
    flow.stop().await;
}

#[tokio::test]
async fn provider_controlled_plugins_are_not_new_or_suppressed_and_user_updates_stay_on() {
    let flow = fixture("known-provider-controlled").await;
    flow.manager.plugin_catalog().unwrap();
    let codex = flow.manager.registry().roots().codex_home.clone();
    let mut config = String::new();
    for (name, market) in [
        ("remote", "openai-curated-remote"),
        ("bundled", "openai-bundled"),
        ("runtime", "openai-primary-runtime"),
        ("user", "user-market"),
    ] {
        let plugin = codex.join(format!("plugins/cache/{market}/{name}/1.0.0"));
        write(
            &plugin.join(".codex-plugin/plugin.json"),
            &format!(r#"{{"name":"{name}"}}"#),
        );
        write(
            &plugin.join(format!("skills/{name}/SKILL.md")),
            &skill_text(name, "one"),
        );
        write(
            &plugin.join(".mcp.json"),
            &format!(r#"{{"mcpServers":{{"{name}":{{"command":"/app/{name}"}}}}}}"#),
        );
        config.push_str(&format!("[plugins.\"{name}@{market}\"]\nenabled = true\n"));
    }
    write(&codex.join("config.toml"), &config);
    let found = changes(&flow).await;
    assert_eq!(names(&found).0, ["user", "user"]);
    let keys = found.found.iter().map(|item| &item.key).collect::<Vec<_>>();
    apply(&flow, &keys, &[], &[], &[]).await;
    write(
        &codex.join("plugins/cache/user-market/user/1.0.0/skills/user/SKILL.md"),
        &skill_text("user", "updated"),
    );
    assert_eq!(changes(&flow).await, PluginChanges::default());
    let view = flow.manager.plugin_catalog().unwrap();
    assert!(
        view.rows
            .iter()
            .all(|row| row.name == "user" && row.choice.on)
    );
    let resolved = flow
        .manager
        .resolve_plugins(
            &flow
                .manager
                .runtime
                .launch_account(ProviderKind::Codex, None),
            Some(&flow.repo),
            &flow.repo,
            true,
            super::super::resolve::Role::Thread,
        )
        .await
        .unwrap();
    assert!(resolved.servers.iter().any(|server| server.name == "user"));
    assert!(resolved.skills.iter().any(|skill| skill.display == "user"));
    for name in ["remote", "bundled", "runtime"] {
        assert!(
            !resolved
                .off
                .servers
                .iter()
                .any(|server| server.contains(name))
        );
        assert!(
            !resolved
                .off
                .plugin_servers
                .iter()
                .any(|(plugin, _)| plugin.starts_with(name))
        );
        assert!(
            !resolved
                .off
                .plugin_skills
                .iter()
                .any(|skill| skill.contains(name))
        );
        assert!(
            !resolved
                .off
                .skill_files
                .iter()
                .any(|file| file.to_string_lossy().contains(name))
        );
    }
    flow.stop().await;
}
