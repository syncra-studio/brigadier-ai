use super::super::cleanup_context::Secrets;
use super::*;
use brigadier_registry::Roots;
use brigadier_registry::library::Library;
use serde_json::json;
use std::cell::Cell;
use std::collections::BTreeMap;

struct NoSecrets;
impl Secrets for NoSecrets {
    fn load(&self, _: &str) -> Result<BTreeMap<String, String>, String> {
        Ok(BTreeMap::new())
    }
}
struct Home {
    path: PathBuf,
    roots: Roots,
    library: Library,
    data: PathBuf,
}
impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("brigadier-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let roots = Roots::from_vars([("HOME".into(), path.display().to_string())]).unwrap();
        Self {
            library: Library::in_home(&path, path.join("brigadier")),
            data: path.join("data"),
            path,
            roots,
        }
    }
    fn write(&self, file: impl AsRef<Path>, text: impl AsRef<[u8]>) -> PathBuf {
        let file = self.path.join(file);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        file
    }
    fn sharing(&self, value: Value) -> PathBuf {
        self.write(
            ".brigadier/library/.sharing.json",
            serde_json::to_vec(&value).unwrap(),
        )
    }
    fn run(&self, import: Import<'_>, runner: Runner<'_>) -> Result<(), String> {
        self.run_with(&self.data, false, import, runner)
    }
    fn run_with(
        &self,
        data: &Path,
        overridden: bool,
        import: Import<'_>,
        runner: Runner<'_>,
    ) -> Result<(), String> {
        let projects = vec![self.path.join("project")];

        let binary = |_: Agent| None;
        let ctx = Ctx {
            roots: &self.roots,
            env: &[],
            library: &self.library,
            secrets: &NoSecrets,
            projects: &projects,
            claude_configs: &[],
            binary: &binary,
        };
        Cleanup {
            data,
            installed_default: &self.data,
            overridden,
            ctx: &ctx,
            codex_homes: &[],
        }
        .run(import, runner)
    }
    fn state(&self) -> Checkpoint {
        read_json(&self.data.join(CHECKPOINT)).unwrap().unwrap()
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
fn no_cli(_: Agent, _: &Path, _: &[Step]) -> Result<(), String> {
    panic!("unexpected real CLI work")
}
fn no_import(_: &Sharing) -> Result<(), String> {
    panic!("missing sharing must not import choices")
}

#[test]
fn cleanup_namespace_covers_user_projects_worktrees_and_preserves_neighbours() {
    let home = Home::new();
    let worktree = home.path.join("session/work");
    let mine_toml = "# my config\nmodel = 'mine' # leave this\n\n[plugins.\"user@elsewhere\"]\nenabled = true # my switch\n";
    let user_config = home.write(".codex/config.toml", format!("{mine_toml}\n[plugins.\"test@brigadier\"]\nenabled = true\n\n[marketplaces.brigadier]\nsource = '/old/library'\n"));
    let mine_json = "\"user@elsewhere\" : { \"custom\" : true }";
    for project in [home.path.join("project"), worktree.clone()] {
        home.write(
            project.join(".codex/config.toml"),
            "[plugins.\"test@brigadier\"]\nenabled = true\n",
        );
        for name in ["settings.json", "settings.local.json"] {
            home.write(project.join(".claude").join(name), format!("{{\n // user comment\n \"enabledPlugins\": {{ {mine_json}, \"test@brigadier\" : true }},\n \"other\": [ 1,  2 ]\n}}\n"));
        }
    }
    let installs = home.write(
        ".claude/plugins/installed_plugins.json",
        json!({"version": 2, "plugins": {
            "test@brigadier": [{"projectPath":worktree,"installPath":"/cache"}],
            "user@elsewhere": [{"projectPath":"/user/location","custom":true}]
        }})
        .to_string(),
    );
    let untouched = home.write(
        "project/.mcp.json",
        b"{\"mcpServers\":{\"mine\":{\"command\":\"mine\"}}}\n",
    );
    let pristine = std::fs::read(&untouched).unwrap();
    home.run(&no_import, &|_, _, steps| {
        let Step::Run(call) = &steps[0] else { panic!() };
        assert_eq!(call.args, ["plugin", "marketplace", "remove", "brigadier"]);
        Ok(())
    })
    .unwrap();
    assert!(finished(&home.data));
    let text = std::fs::read_to_string(user_config).unwrap();
    assert!(text.starts_with(mine_toml), "{text}");
    assert!(!text.contains("brigadier"));
    for project in [home.path.join("project"), worktree] {
        assert!(!project.join(".codex/config.toml").exists());
        for name in ["settings.json", "settings.local.json"] {
            let text = std::fs::read_to_string(project.join(".claude").join(name)).unwrap();
            assert!(text.contains(mine_json));
            assert!(text.contains("// user comment"));
            assert!(text.contains("\"other\": [ 1,  2 ]"));
            assert!(!text.contains("@brigadier"));
        }
    }
    assert!(
        !std::fs::read_to_string(installs)
            .unwrap()
            .contains("@brigadier")
    );
    assert_eq!(std::fs::read(untouched).unwrap(), pristine);
    home.run(&no_import, &no_cli).unwrap();
}

#[test]
fn cleanup_import_is_durable_before_restore_and_old_choices_are_untouched() {
    let home = Home::new();
    let config = home.write(
        ".codex/config.toml",
        "# keep\n[mcp_servers.mine]\ncommand = 'mine'\nenabled = false\n",
    );
    let original = json!({"chosen":{"server:mine":{"on":false,"scope":[home.path.join("project")]}},"offScopes":{"server:mine":[home.path.join("project")]}, "declined":[{"kind":"skill","name":"removed"}],
        "owned":[{"owner":"server:mine","file":config,"folder":null,"name":"mine","claude":false,"copy":false,"before":null,"after":false}]});
    let sharing = home.sharing(original);
    let before = std::fs::read(&sharing).unwrap();
    let imported = Cell::new(false);
    home.run(
        &|old| {
            assert_eq!(std::fs::read(&sharing).unwrap(), before);
            assert!(
                std::fs::read_to_string(&config)
                    .unwrap()
                    .contains("enabled = false")
            );
            assert!(serde_json::to_value(old).unwrap()["chosen"]["server:mine"].is_object());
            save(
                &home.data.join("plugins.json"),
                &json!({"choices":"durable"}),
            )?;
            imported.set(true);
            Ok(())
        },
        &no_cli,
    )
    .unwrap();
    assert!(imported.get());
    assert!(
        !std::fs::read_to_string(&config)
            .unwrap()
            .contains("enabled")
    );
    assert_eq!(std::fs::read(sharing).unwrap(), before);
    assert!(home.data.join("plugins.json").exists());
}

#[test]
fn cleanup_restores_claude_flags_but_respects_user_changes_to_codex_flags() {
    let home = Home::new();
    let project = home.path.join("project");
    let claude = home.write(".claude.json", format!("{{\"custom\": [ 1, 2 ], \"projects\": {{{}: {{\"disabledMcpServers\":[\"mine\",\"user-off\"]}}}}}}", serde_json::to_string(&project).unwrap()));
    let codex = home.write(
        ".codex/config.toml",
        "[mcp_servers.mine]\ncommand='mine'\nenabled=true # changed by user\n",
    );
    let codex_before = std::fs::read(&codex).unwrap();
    home.sharing(json!({"owned":[
        {"owner":"server:mine","file":claude,"folder":project,"name":"mine","claude":true,"copy":false,"before":false,"after":true},
        {"owner":"server:mine","file":codex,"folder":null,"name":"mine","claude":false,"copy":false,"before":null,"after":false}
    ]}));
    home.run(&|_| Ok(()), &no_cli).unwrap();
    let text = std::fs::read_to_string(claude).unwrap();
    assert!(text.contains("\"custom\": [ 1, 2 ]"));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["projects"][project.display().to_string()]["disabledMcpServers"],
        json!(["user-off"])
    );
    assert_eq!(std::fs::read(codex).unwrap(), codex_before);
}

#[test]
fn cleanup_interruption_retries_only_incomplete_steps_and_remembers_worktrees() {
    let home = Home::new();
    home.sharing(json!({}));
    let project = home.path.join("worktree");
    home.write(
        project.join(".claude/settings.local.json"),
        "{\"enabledPlugins\":{\"mine@brigadier\":true}}",
    );
    home.write(
        ".claude/plugins/installed_plugins.json",
        json!({"plugins":{"mine@brigadier":[{"projectPath":project}]}}).to_string(),
    );
    home.write(
        ".claude/plugins/known_marketplaces.json",
        "{\"brigadier\":{\"source\":\"old\"}}",
    );
    let imported = Cell::new(0);
    let import = |_: &Sharing| {
        imported.set(imported.get() + 1);
        Ok(())
    };
    let error = home
        .run(&import, &|_, _, _| Err("interrupted CLI removal".into()))
        .unwrap_err();
    assert!(error.contains("interrupted"));
    assert!(!finished(&home.data));
    assert_eq!(home.state().completed, BTreeSet::from([1, 2, 3]));
    // CLI records have gone already; the next attempt still has their project paths.
    assert!(home.state().projects.contains(&project));
    home.write(
        project.join(".claude/settings.local.json"),
        "{\"enabledPlugins\":{\"mine@brigadier\":true}}",
    );
    home.run(&import, &|_, _, _| Ok(())).unwrap();
    assert!(!project.join(".claude/settings.local.json").exists());
    assert_eq!(imported.get(), 1);
    assert!(finished(&home.data));
}

#[test]
fn cleanup_corrupt_file_is_reported_untouched_and_retried() {
    let home = Home::new();
    let broken = home.write("project/.claude/settings.json", "{ broken JSON\n");
    home.write(
        "project/.claude/settings.local.json",
        "{\"enabledPlugins\":{\"test@brigadier\":true}}",
    );
    let error = home.run(&no_import, &no_cli).unwrap_err();
    assert!(error.contains(&broken.display().to_string()));
    assert_eq!(std::fs::read_to_string(&broken).unwrap(), "{ broken JSON\n");
    assert!(
        !home
            .path
            .join("project/.claude/settings.local.json")
            .exists()
    );
    assert!(!finished(&home.data));
    assert!(!home.state().errors.is_empty());
    home.write(&broken, "{\"mine\": true}");
    home.run(&no_import, &no_cli).unwrap();
    assert!(finished(&home.data));
    assert_eq!(std::fs::read_to_string(broken).unwrap(), "{\"mine\": true}");
}

#[test]
fn cleanup_data_dir_override_and_nondefault_are_noops() {
    let home = Home::new();
    let config = home.write(
        ".codex/config.toml",
        "[plugins.\"mine@brigadier\"]\nenabled=true\n",
    );
    let before = std::fs::read(&config).unwrap();
    home.run_with(&home.data, true, &no_import, &no_cli)
        .unwrap();
    home.run_with(&home.path.join("dev-data"), false, &no_import, &no_cli)
        .unwrap();
    assert_eq!(std::fs::read(config).unwrap(), before);
    assert!(!home.data.exists());
}

#[test]
fn cleanup_without_sharing_runs_only_namespace_step() {
    let home = Home::new();
    let kept = home.write(".brigadier/library/.moved/saved-skill", "original");
    home.write(
        ".brigadier/library/.moved/old.json",
        json!({"originals":[{"kind":"skill","from":home.path.join("restored"),"kept":kept}]})
            .to_string(),
    );
    let exclude = home.write(
        "project/.git/info/exclude",
        "# mine\n/.codex/config.toml\n/.claude/settings.local.json\n",
    );
    home.run(&no_import, &no_cli).unwrap();
    assert_eq!(home.state().completed, BTreeSet::from([4]));
    assert!(kept.exists());
    assert!(!home.path.join("restored").exists());
    assert!(!home.data.join("library").exists());
    assert_eq!(
        std::fs::read_to_string(exclude).unwrap(),
        "# mine\n/.codex/config.toml\n/.claude/settings.local.json\n"
    );
}

#[test]
fn cleanup_library_only_items_and_legacy_moved_original_survive_cli_removal() {
    let home = Home::new();
    home.sharing(json!({}));
    let manifest = home.write(
        ".brigadier/library/.claude-plugin/marketplace.json",
        "{\"plugins\":[{\"name\":\"library-only\"}]}",
    );
    let skill = home.write(
        ".brigadier/library/plugins/library-only/skills/mine/SKILL.md",
        "user added skill\n",
    );
    let kept = home.write(
        ".brigadier/library/.moved/kept-skill/SKILL.md",
        "user original\n",
    );
    let from = home.path.join(".agents/skills/mine");
    home.write(
        ".brigadier/library/.moved/old.json",
        json!({"originals":[{"kind":"skill","from":from,"kept":kept.parent().unwrap()}]})
            .to_string(),
    );
    home.run(&|_| Ok(()), &no_cli).unwrap();
    assert_eq!(
        std::fs::read(
            home.data
                .join("library/plugins/library-only/skills/mine/SKILL.md")
        )
        .unwrap(),
        std::fs::read(skill).unwrap()
    );
    assert_eq!(
        std::fs::read(home.data.join("library/.claude-plugin/marketplace.json")).unwrap(),
        std::fs::read(manifest).unwrap()
    );
    assert_eq!(
        std::fs::read(from.join("SKILL.md")).unwrap(),
        b"user original\n"
    );
}

#[test]
fn cleanup_import_failure_prevents_all_later_mutations() {
    let home = Home::new();
    let sharing = home.sharing(json!({}));
    let config = home.write(
        ".codex/config.toml",
        "[plugins.\"mine@brigadier\"]\nenabled=true\n",
    );
    let before = std::fs::read(&config).unwrap();
    let old = std::fs::read(&sharing).unwrap();
    assert!(
        home.run(&|_| Err("cannot sync choices".into()), &no_cli)
            .is_err()
    );
    assert_eq!(std::fs::read(config).unwrap(), before);
    assert_eq!(std::fs::read(sharing).unwrap(), old);
    assert!(home.state().completed.is_empty());
}

#[test]
fn cleanup_user_only_configs_are_byte_identical() {
    let home = Home::new();
    let cases = [
        (
            ".codex/config.toml",
            "# user's spacing\r\nmodel = 'custom' # keep\r\n[plugins.\"user@mine\"]\r\nenabled=true\r\n",
        ),
        (
            "project/.claude/settings.json",
            "{\r\n // user's comment\r\n \"enabledPlugins\" : {\"user@mine\":true,},\r\n \"other\" : [1,  2,],\r\n}\r\n",
        ),
        (
            ".claude/plugins/installed_plugins.json",
            "{\"version\":2,\"plugins\":{\"user@mine\" : [{\"projectPath\": \"/keep\"}]}}\n",
        ),
    ];
    for (file, text) in cases {
        home.write(file, text);
    }
    home.run(&no_import, &no_cli).unwrap();
    for (file, text) in cases {
        assert_eq!(
            std::fs::read(home.path.join(file)).unwrap(),
            text.as_bytes()
        );
    }
}

#[test]
fn cleanup_json_splices_all_member_positions_and_keeps_comments() {
    let home = Home::new();
    for (i, members) in [
        "\"a@brigadier\":true, /* keep */ \"mine\":false",
        "\"mine\":false, /* keep */ \"a@brigadier\":true",
        "\"a@brigadier\":true, \"mine\":false, /* keep */ \"b@brigadier\":false,",
        "\"mine\":false, \"a@brigadier\":true, \"b@brigadier\":false /* keep */",
        "\"a@brigadier\":true, \"b@brigadier\":false, /* keep */ \"mine\":false",
    ]
    .iter()
    .enumerate()
    {
        let file = home.write(
            format!("project/{i}.json"),
            format!("{{\"enabledPlugins\":{{{members}}},\"unicode\":\"λ日本\"}}"),
        );
        clean_json(&file, "enabledPlugins").unwrap();
        let text = std::fs::read_to_string(file).unwrap();
        assert!(text.contains("/* keep */"));
        assert!(text.contains("\"mine\":false"));
        assert!(text.contains("\"unicode\":\"λ日本\""));
        assert!(!text.contains("@brigadier"));
        Json::parse(&text).unwrap();
    }
}

#[test]
fn cleanup_marketplace_removal_runs_after_library_preservation_and_is_checkpointed() {
    let home = Home::new();
    let sharing = home.sharing(json!({}));
    let old = std::fs::read(&sharing).unwrap();
    home.write(
        ".brigadier/library/.claude-plugin/marketplace.json",
        "{\"plugins\":[{\"name\":\"only\"}]}",
    );
    home.write(
        ".brigadier/library/plugins/only/skills/mine/SKILL.md",
        "mine",
    );
    home.write(
        ".codex/config.toml",
        "[marketplaces.brigadier]\nsource='old'\n",
    );
    home.write(
        ".claude/plugins/known_marketplaces.json",
        "{\"brigadier\":{\"source\":\"old\"},\"mine\":{}}",
    );
    let calls = Cell::new(0);
    let runner = |agent, dir: &Path, steps: &[Step]| {
        assert!(
            home.data
                .join("library/plugins/only/skills/mine/SKILL.md")
                .exists()
        );
        assert_eq!(std::fs::read(&sharing).unwrap(), old);
        let Step::Run(call) = &steps[0] else { panic!() };
        assert_eq!(call.args, ["plugin", "marketplace", "remove", "brigadier"]);
        match agent {
            Agent::Claude => assert_eq!(dir, home.roots.claude_dir),
            Agent::Codex => {
                assert_eq!(dir, home.roots.codex_home);
                assert!(
                    std::fs::read_to_string(dir.join("config.toml"))
                        .unwrap()
                        .contains("marketplaces.brigadier")
                );
            }
        }
        calls.set(calls.get() + 1);
        Ok(())
    };
    home.run(&|_| Ok(()), &runner).unwrap();
    assert_eq!(calls.get(), 2);
    assert!(!home.path.join(".codex/config.toml").exists());
    home.run(&|_| panic!("must not reimport"), &no_cli).unwrap();
}

#[test]
fn cleanup_corrupt_sharing_cannot_authorize_restore_or_deletion() {
    let home = Home::new();
    let sharing = home.write(".brigadier/library/.sharing.json", "{truncated");
    let file = home.write(
        ".codex/config.toml",
        "[plugins.\"mine@brigadier\"]\nenabled=true\n",
    );
    let before = std::fs::read(&file).unwrap();
    let error = home.run(&no_import, &no_cli).unwrap_err();
    assert!(error.contains(&sharing.display().to_string()));
    assert_eq!(std::fs::read(file).unwrap(), before);
    assert!(!finished(&home.data));
}

#[test]
fn cleanup_toml_comments_keep_an_otherwise_empty_file() {
    let home = Home::new();
    let text = "# user's comment\n[plugins.\"only@brigadier\"] # keep this too\nenabled = true # keep inline\n";
    let file = home.write("project/.codex/config.toml", text);
    home.run(&no_import, &no_cli).unwrap();
    let after = std::fs::read_to_string(file).unwrap();
    for comment in ["# user's comment", "# keep this too", "# keep inline"] {
        assert!(after.contains(comment), "{after}");
    }
    assert!(!after.contains("@brigadier"));
    assert!(after.parse::<DocumentMut>().unwrap().is_empty());
    // Hashes in multiline strings are data, not comments to retain after deletion.
    assert_eq!(
        toml_comments("value = '''\n# data\n''' # actual\n"),
        vec!["# actual"]
    );
}

#[test]
fn cleanup_actual_choices_import_precedes_flag_restore() {
    let home = Home::new();
    let config = home.write(
        ".codex/config.toml",
        "[mcp_servers.mine]\ncommand = 'mine'\nenabled = false\n",
    );
    let sharing = home.sharing(json!({"chosen":{"server:mine":{"on":false,"scope":[home.path.join("project")]}},
        "owned":[{"owner":"server:mine","file":config,"folder":null,"name":"mine","claude":false,"copy":false,"before":null,"after":false}]}));
    let rows = Catalog::rows(&[Catalog::read(&home.roots, None)]);
    assert!(!rows.is_empty());
    home.run(
        &|_| {
            assert!(
                std::fs::read_to_string(&config)
                    .unwrap()
                    .contains("enabled = false")
            );
            super::super::choices::import_sharing_file(&home.data, &sharing, &rows).map(|_| ())
        },
        &no_cli,
    )
    .unwrap();
    let choices = super::super::choices::Choices::load(&home.data).unwrap();
    assert!(!choices.choice(&rows[0]).on);
    assert!(
        choices
            .choice(&rows[0])
            .scope
            .contains(Some(&home.path.join("project")))
    );
    assert!(
        !choices
            .choice(&rows[0])
            .scope
            .contains(Some(&home.path.join("elsewhere")))
    );
    assert!(!std::fs::read_to_string(config).unwrap().contains("enabled"));
}

#[test]
fn cleanup_handles_divergent_account_homes_and_their_worktree_records() {
    let home = Home::new();
    let claude = home.path.join("accounts/claude");
    let codex = home.path.join("accounts/codex");
    let project = home.path.join("accounts-worktree");
    home.write(
        claude.join("plugins/installed_plugins.json"),
        json!({"plugins":{"only@brigadier":[{"projectPath":project}]}}).to_string(),
    );
    home.write(
        claude.join("plugins/known_marketplaces.json"),
        "{\"brigadier\":{}}",
    );
    home.write(
        claude.join("settings.json"),
        "{\"enabledPlugins\":{\"only@brigadier\":true}}",
    );
    home.write(
        codex.join("config.toml"),
        "[plugins.\"only@brigadier\"]\nenabled=true\n[marketplaces.brigadier]\nsource='old'\n",
    );
    home.write(
        project.join(".claude/settings.json"),
        "{\"enabledPlugins\":{\"only@brigadier\":true}}",
    );
    let configs = [claude.join(".claude.json")];
    let codex_homes = [codex.clone()];

    let binary = |_: Agent| None;
    let ctx = Ctx {
        roots: &home.roots,
        env: &[],
        library: &home.library,
        secrets: &NoSecrets,
        projects: &[],
        claude_configs: &configs,
        binary: &binary,
    };
    let calls = Cell::new(0);
    Cleanup {
        data: &home.data,
        installed_default: &home.data,
        overridden: false,
        ctx: &ctx,
        codex_homes: &codex_homes,
    }
    .run(&no_import, &|agent, dir, _| {
        assert_eq!(
            dir,
            match agent {
                Agent::Claude => &claude,
                Agent::Codex => &codex,
            }
        );
        calls.set(calls.get() + 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert!(!claude.join("settings.json").exists());
    assert!(!codex.join("config.toml").exists());
    assert!(!project.join(".claude/settings.json").exists());
    assert!(home.state().projects.contains(&project));
}

#[test]
fn cleanup_partial_original_restore_resumes_without_reimporting() {
    let home = Home::new();
    home.sharing(json!({}));
    let first = home.write(".brigadier/library/.moved/first/SKILL.md", "first original");
    let second = home.write(
        ".brigadier/library/.moved/second/SKILL.md",
        "second original",
    );
    let first_from = home.path.join(".agents/skills/first");
    let second_from = home.write(".agents/skills/second", "user replacement");
    home.write(
        ".brigadier/library/.moved/record.json",
        json!({"originals":[
            {"kind":"skill","from":first_from,"kept":first.parent().unwrap()},
            {"kind":"skill","from":second_from,"kept":second.parent().unwrap()}
        ]})
        .to_string(),
    );
    let error = home.run(&|_| Ok(()), &no_cli).unwrap_err();
    assert!(error.contains("already there"));
    assert_eq!(home.state().completed, BTreeSet::from([1, 2]));
    assert_eq!(home.state().restored.len(), 1);
    assert_eq!(
        std::fs::read(first_from.join("SKILL.md")).unwrap(),
        b"first original"
    );
    assert_eq!(std::fs::read(&second_from).unwrap(), b"user replacement");
    std::fs::remove_file(&second_from).unwrap();
    home.run(&|_| panic!("already imported"), &no_cli).unwrap();
    assert_eq!(
        std::fs::read(second_from.join("SKILL.md")).unwrap(),
        b"second original"
    );
    assert!(finished(&home.data));
}

#[test]
fn cleanup_finds_projects_remembered_only_by_old_scope_choices() {
    let home = Home::new();
    let project = home.path.join("unregistered-project");
    home.sharing(json!({"offScopes":{"server:mine":[project]}}));
    let file = home.write(
        project.join(".codex/config.toml"),
        "[plugins.\"only@brigadier\"]\nenabled=true\n",
    );
    home.run(&|_| Ok(()), &no_cli).unwrap();
    assert!(!file.exists());
    assert!(home.state().projects.contains(&project));
}

#[cfg(unix)]
#[test]
fn cleanup_preserves_user_symlinks_while_editing_their_target() {
    let home = Home::new();
    let target = home.write(
        "user-config.json",
        "{\"enabledPlugins\":{\"only@brigadier\":true}, \"user\" : [ 1,2 ]}",
    );
    std::fs::create_dir_all(home.path.join("project/.claude")).unwrap();
    let link = home.path.join("project/.claude/settings.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    home.run(&no_import, &no_cli).unwrap();
    assert_eq!(std::fs::read_link(link).unwrap(), target);
    let text = std::fs::read_to_string(target).unwrap();
    assert!(!text.contains("brigadier"));
    assert!(text.contains("\"user\" : [ 1,2 ]"));
}

#[test]
fn cleanup_json_comments_inside_removed_entries_keep_the_file() {
    let home = Home::new();
    let file = home.write(
        "project/.claude/settings.json",
        "{\"enabledPlugins\": {\"only@brigadier\": /* keep this */ true,\n// keep this too\n}}\n",
    );
    home.run(&no_import, &no_cli).unwrap();
    let after = std::fs::read_to_string(file).unwrap();
    assert!(after.contains("/* keep this */"));
    assert!(after.contains("// keep this too\n"));
    assert!(!after.contains("@brigadier"));
    assert!(Json::parse(&after).unwrap().members().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn cleanup_library_destination_symlink_is_reported_without_following_it() {
    let home = Home::new();
    home.sharing(json!({}));
    home.write(
        ".brigadier/library/.claude-plugin/marketplace.json",
        "{\"plugins\":[{\"name\":\"only\"}]}",
    );
    home.write(".brigadier/library/plugins/only/SKILL.md", "library skill");
    let external = home.write("external/SKILL.md", "user data");
    std::fs::create_dir_all(home.data.join("library/plugins")).unwrap();
    std::os::unix::fs::symlink(
        external.parent().unwrap(),
        home.data.join("library/plugins/only"),
    )
    .unwrap();
    let error = home.run(&|_| Ok(()), &no_cli).unwrap_err();
    assert!(error.contains("symlink"));
    assert_eq!(std::fs::read(external).unwrap(), b"user data");
    assert!(!finished(&home.data));
}

// Keep the legacy native-server restore regression after retiring ops' old writers.
#[test]
fn cleanup_legacy_server_restore_is_idempotent_and_keeps_saved_values() {
    struct Saved;
    impl Secrets for Saved {
        fn load(&self, _: &str) -> Result<BTreeMap<String, String>, String> {
            Ok(BTreeMap::from([("TOKEN".into(), "saved-value".into())]))
        }
    }
    let home = Home::new();
    let project = home.path.join("project");
    home.write(
        ".codex/config.toml",
        format!(
            "[projects.{}]\ntrust_level='trusted'\n",
            serde_json::to_string(&project).unwrap()
        ),
    );
    let projects = [project.clone()];
    let ctx = Ctx {
        roots: &home.roots,
        env: &[],
        library: &home.library,
        secrets: &Saved,
        projects: &projects,
        claude_configs: &[],
        binary: &|_| None,
    };
    let original = Original::Server {
        agent: ProviderKind::Codex,
        name: "legacy".into(),
        project: Some(project.clone()),
        transport: SavedTransport::Stdio {
            command: "/opt/mcp".into(),
            args: vec!["two words".into()],
            env: vec!["TOKEN".into()],
            cwd: None,
        },
        secrets: "legacy/original-0".into(),
    };
    let restore = |agent, _: &Path, steps: &[Step]| {
        assert_eq!(agent, Agent::Codex);
        // Project-file restoration executes no agent CLI; use the actual migrated editor.
        assert!(
            steps
                .iter()
                .all(|step| matches!(step, Step::ProjectFile { .. }))
        );
        brigadier_registry::adapter::apply(
            adapter(agent),
            &home.roots,
            &Catalog::read(&home.roots, None),
            steps,
            &[],
            None,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
    original.restore(&ctx, &restore).unwrap();
    let file = project.join(".codex/config.toml");
    let before = std::fs::read(&file).unwrap();
    original.restore(&ctx, &restore).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(
        Saved.load("legacy/original-0").unwrap()["TOKEN"],
        "saved-value"
    );
    home.write(
        &file,
        "[mcp_servers.legacy]\ncommand='somebody-elses-command'\n",
    );
    assert!(original.restore(&ctx, &restore).is_err());
    assert!(
        std::fs::read_to_string(file)
            .unwrap()
            .contains("somebody-elses-command")
    );
}
