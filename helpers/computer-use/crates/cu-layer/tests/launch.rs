//! Brigadier's `launch_app`: its schema, and the parts of a launch that
//! decide without launching anything.
#![cfg(target_os = "macos")]

use std::collections::HashSet;
use std::path::PathBuf;

use cu_layer::launch::{
    self, apps, local_file_target, names_app, preflight_file_urls, put_window_first, retry_launch,
    settings_hint, split_open_targets, watchdog_ticks, LaunchAppTool,
};
use cu_layer::target_policy::{self, TargetResolver};
use cua_driver_core::tool::Tool;
use platform_macos::windows::WindowInfo;
use serde_json::json;

#[test]
fn the_schema_is_the_earlier_tools_without_display() {
    let def = LaunchAppTool.def();
    assert_eq!(def.name, "launch_app");
    assert!(!def.read_only && !def.destructive && def.idempotent && def.open_world);
    assert_eq!(
        def.input_schema,
        json!({
            "type": "object",
            "properties": {
                "bundle_id": {
                    "type": "string",
                    "description": "App bundle identifier, e.g. com.apple.calculator. Preferred over name."
                },
                "name": {
                    "type": "string",
                    "description": "App display name, or the full path to its .app bundle. Used only when bundle_id is absent."
                },
                "urls": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional file paths or URLs to open with the app (e.g. a folder path for Finder)."
                },
                "cdp_debugging_port": {
                    "type": "integer",
                    "description": "Open a Chrome DevTools Protocol server on this port (appends --remote-debugging-port=N)."
                },
                "webkit_inspector_port": {
                    "type": "integer",
                    "description": "Open a WebKit inspector server on this port (sets WEBKIT_INSPECTOR_SERVER env var)."
                },
                "creates_new_application_instance": {
                    "type": "boolean",
                    "description": "When true, force a new app instance even if already running (open -n). Use for concurrent multi-agent/multi-session work so each session gets an isolated instance + window instead of sharing one — on single-instance apps (e.g. Calculator) every caller otherwise gets the same window and the sessions clobber each other."
                },
                "additional_arguments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra arguments appended after --args when launching."
                }
            },
            "additionalProperties": false
        })
    );
    assert!(!def.description.contains("display"));
    assert!(!def.description.contains("placement"));
    for kept in [
        "cdp_debugging_port",
        "webkit_inspector_port",
        "creates_new_application_instance",
        "additional_arguments",
        "self_activation_suppressed",
        "new background window of its own, listed first",
    ] {
        assert!(def.description.contains(kept), "{kept}");
    }
}

#[test]
fn installing_puts_brigadiers_launch_app_in_place_of_the_drivers() {
    let mut registry = platform_macos::register_tools();
    launch::install(&mut registry);
    let list = registry.tools_list();
    let tool = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "launch_app")
        .expect("launch_app is listed");
    let properties = tool["inputSchema"]["properties"].as_object().unwrap();
    assert!(properties.contains_key("cdp_debugging_port"));
    assert!(!properties.contains_key("display"));
}

fn window(window_id: u32) -> WindowInfo {
    serde_json::from_value(json!({
        "window_id": window_id,
        "pid": 1,
        "app_name": "App",
        "title": "",
        "bounds": { "x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0 },
        "layer": 0,
        "z_index": 0,
        "is_on_screen": true,
        "on_current_space": null,
        "space_ids": null,
    }))
    .unwrap()
}

fn ids(windows: &[WindowInfo]) -> Vec<u32> {
    windows.iter().map(|w| w.window_id).collect()
}

#[test]
fn the_main_window_is_listed_first_and_the_rest_keep_their_order() {
    let mut windows = vec![window(1), window(2), window(3), window(4)];
    put_window_first(&mut windows, Some(3));
    assert_eq!(ids(&windows), [3, 1, 2, 4]);

    let mut windows = vec![window(1), window(2)];
    put_window_first(&mut windows, Some(9));
    assert_eq!(
        ids(&windows),
        [1, 2],
        "an unknown main window changes nothing"
    );
    put_window_first(&mut windows, None);
    assert_eq!(ids(&windows), [1, 2]);
}

#[test]
fn a_documents_new_window_is_listed_first() {
    let before: HashSet<u32> = [1, 2].into();
    let mut windows = vec![window(1), window(5), window(2)];
    let new = cu_layer::browser::open_link::new_windows_first(&mut windows, &before);
    assert_eq!(new, 1);
    assert_eq!(ids(&windows), [5, 1, 2]);
}

#[test]
fn a_launch_is_tried_again_while_an_earlier_instance_is_still_quitting() {
    // The first failure is always tried again once.
    assert!(retry_launch(false, false, false));
    assert!(retry_launch(false, false, true));
    // After that, only while the app is still listed (quitting).
    assert!(retry_launch(false, true, true));
    assert!(!retry_launch(false, true, false));
    // Never past the deadline.
    assert!(!retry_launch(true, false, true));
    assert!(!retry_launch(true, true, true));
}

#[test]
fn a_running_app_is_matched_by_bundle_id_else_by_name() {
    let calculator = (Some("com.apple.calculator"), "Calculator");
    assert!(names_app(
        calculator.0,
        calculator.1,
        Some("com.apple.calculator"),
        None
    ));
    // The bundle id wins over the name.
    assert!(!names_app(
        calculator.0,
        calculator.1,
        Some("com.apple.TextEdit"),
        Some("Calculator")
    ));
    assert!(names_app(
        calculator.0,
        calculator.1,
        None,
        Some("Calculator")
    ));
    assert!(!names_app(
        calculator.0,
        calculator.1,
        None,
        Some("TextEdit")
    ));
    assert!(!names_app(calculator.0, calculator.1, None, None));
}

#[test]
fn an_app_started_from_scratch_is_kept_in_the_background_as_long_as_one_handed_documents() {
    assert_eq!(watchdog_ticks(false, true), watchdog_ticks(true, false));
    assert_eq!(watchdog_ticks(true, true), watchdog_ticks(true, false));
    // ~10.5 s: past an Electron app's own activation once its window is ready.
    assert!(watchdog_ticks(false, true) * 250 >= 10_000);
    // An app already running reacts at once; no long hold on the user's clicks.
    assert!(watchdog_ticks(false, false) * 250 <= 500);
}

#[test]
fn files_and_links_among_launch_arguments_are_told_apart_from_flags() {
    let (targets, rest) = split_open_targets(
        [
            "--user-data-dir=/tmp/profile",
            "--no-first-run",
            "file:///tmp/page.html",
            "/tmp/doc.txt",
            "~/notes.txt",
            "https://example.com",
            "plain",
        ]
        .map(String::from)
        .to_vec(),
    );
    assert_eq!(
        targets,
        [
            "file:///tmp/page.html",
            "/tmp/doc.txt",
            "~/notes.txt",
            "https://example.com"
        ]
    );
    assert_eq!(
        rest,
        ["--user-data-dir=/tmp/profile", "--no-first-run", "plain"]
    );
}

#[test]
fn local_file_target_treats_plain_paths_as_files() {
    assert_eq!(
        local_file_target("/tmp/does-not-exist.md"),
        Some(PathBuf::from("/tmp/does-not-exist.md"))
    );
    assert_eq!(
        local_file_target("relative/path.md"),
        Some(PathBuf::from("relative/path.md"))
    );
}

#[test]
fn local_file_target_skips_remote_and_custom_schemes() {
    assert_eq!(local_file_target("https://example.com"), None);
    assert_eq!(local_file_target("about:blank"), None);
    assert_eq!(local_file_target("myapp://open/item"), None);
    assert_eq!(
        local_file_target("x-apple.systempreferences:com.apple.SystemProfiler.AboutExtension"),
        None
    );
}

#[test]
fn local_file_target_drops_anchor_and_query_from_file_urls() {
    assert_eq!(
        local_file_target("file:///tmp/page.html#bottom"),
        Some(PathBuf::from("/tmp/page.html"))
    );
    assert_eq!(
        local_file_target("file:///tmp/page.html?q=1#x"),
        Some(PathBuf::from("/tmp/page.html"))
    );
}

#[test]
fn local_file_target_percent_decodes_file_urls_before_path_checks() {
    assert_eq!(
        local_file_target("file:///tmp/My%20Doc.txt"),
        Some(PathBuf::from("/tmp/My Doc.txt"))
    );
    assert_eq!(
        local_file_target("file://localhost/tmp/%E2%9C%93.txt"),
        Some(PathBuf::from("/tmp/✓.txt"))
    );
}

#[test]
fn preflight_file_urls_returns_structured_file_not_found() {
    let missing = "/tmp/brigadier-cu-definitely-missing-file-for-test.md".to_string();
    let result = preflight_file_urls(&[missing]).expect("missing file should error");
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["error"], "FILE_NOT_FOUND");
    assert_eq!(
        structured["path"],
        "/tmp/brigadier-cu-definitely-missing-file-for-test.md"
    );
    assert!(structured.get("details").is_none());
    assert!(preflight_file_urls(&["https://example.com".into()]).is_none());
}

#[test]
fn an_app_is_found_by_the_full_path_to_its_bundle_wherever_it_lives() {
    let root = std::env::temp_dir().join(format!("brigadier-launch-test-{}", std::process::id()));
    let bundle = root.join("Somewhere Else.app");
    std::fs::create_dir_all(&bundle).unwrap();
    let path = bundle.to_str().unwrap().to_owned();
    let located = apps::locate_by_name(&path);
    std::fs::remove_dir_all(&root).unwrap();
    match located {
        Some(apps::AppLocator::Path(found)) => assert_eq!(found, path),
        _ => panic!("the bundle at {path} was not found by its path"),
    }
}

#[test]
fn finder_and_the_other_service_apps_are_searched_by_name() {
    let roots = apps::search_roots("/Users/someone");
    assert_eq!(roots[0], "/Applications");
    assert!(roots.contains(&"/Users/someone/Applications".to_owned()));
    let core_services = roots
        .iter()
        .position(|root| root == "/System/Library/CoreServices")
        .expect("macOS's service apps are searched");
    assert!(
        core_services > roots.iter().position(|r| r == "/Applications").unwrap(),
        "the Applications folders come first"
    );
    assert!(roots.contains(&"/System/Library/CoreServices/Applications".to_owned()));
    // On this Mac, Finder is found by its name alone (no launch).
    match apps::locate_by_name("Finder") {
        Some(apps::AppLocator::Path(path)) => {
            assert_eq!(path, "/System/Library/CoreServices/Finder.app")
        }
        _ => panic!("Finder was not found by name"),
    }
}

#[test]
fn system_settings_without_a_pane_answers_how_to_open_one() {
    let hint = settings_hint("com.apple.systempreferences", true).expect("a hint");
    assert!(hint.contains("never clicked or typed into"));
    assert!(hint.contains("urls [\"x-apple.systempreferences:<pane id>\"]"));
    assert!(hint.contains("com.apple.SystemProfiler.AboutExtension (General › About"));
    assert_eq!(settings_hint("com.apple.systempreferences", false), None);
    assert_eq!(settings_hint("com.apple.calculator", true), None);
    assert_eq!(settings_hint("?", true), None);
}

#[test]
fn the_system_settings_hint_says_what_the_refusal_says() {
    let resolver = TargetResolver {
        app_for_pid: &|_| {
            Some(target_policy::TargetIdentity {
                name: "System Settings".into(),
                bundle_id: Some("com.apple.systempreferences".into()),
                pid: Some(106),
            })
        },
        pid_for_window: &|_| None,
    };
    let refused = target_policy::enforce("click", &json!({"pid": 106}), &[], &resolver)
        .expect_err("System Settings can't be clicked");
    let refusal = serde_json::to_string(&refused.content).unwrap();
    let hint = settings_hint("com.apple.systempreferences", true).unwrap();
    for sentence in [
        "System Settings can be opened and read, never clicked or typed into.",
        "To read a pane, open it directly: launch_app with bundle_id com.apple.systempreferences and urls",
        "x-apple.systempreferences:<pane id>",
        "com.apple.SystemProfiler.AboutExtension (General › About",
    ] {
        assert!(hint.contains(sentence), "hint: {sentence}");
        let escaped = serde_json::to_string(sentence).unwrap();
        let escaped = escaped.trim_matches('"');
        assert!(refusal.contains(escaped), "refusal: {sentence}");
    }
}
