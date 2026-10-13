use crate::browser::open_link::OpenedBy;
use async_trait::async_trait;
use cu_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::path::PathBuf;

pub struct LaunchAppTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "launch_app".into(),
        description:
            "Launch a macOS app in the background — the target does NOT come to the foreground.\n\n\
             Provide either `bundle_id` (preferred — unambiguous, e.g. `com.apple.calculator`) \
             or `name` (e.g. \"Calculator\"). If both are given, bundle_id wins.\n\n\
             Optional `urls` are handed to the app as open targets — for Finder, pass a folder \
             path to open a backgrounded Finder window there. A web browser that is already \
             running opens web links and local pages in a new background window of its own, listed first in \
             `windows`.\n\n\
             Optional `cdp_debugging_port`: opens a Chrome DevTools Protocol (CDP) server \
             on the specified port (appends --remote-debugging-port=N to the app's argv). \
             Use this to automate Electron/VS Code/Cursor, or Chrome/Brave/Edge, via CDP.\n\n\
             Optional `webkit_inspector_port`: opens a WebKit inspector server on the specified \
             port (sets WEBKIT_INSPECTOR_SERVER=127.0.0.1:N + TAURI_WEBVIEW_AUTOMATION=1). \
             Use this for Tauri/WebKit-based apps.\n\n\
             Optional `creates_new_application_instance`: when true, forces a new app instance \
             even if one is already running (passes -n to open). Reach for this when another \
             agent or session may drive the SAME app concurrently — it returns a fresh pid + \
             window so each session acts on its own isolated window instead of clobbering one \
             shared instance. Without it, single-instance apps (Calculator, many utilities) hand \
             every caller the same window, so two sessions fight over it.\n\n\
             Optional `additional_arguments`: extra argv strings appended after --args.\n\n\
             Optional `display`: where to put windows this call opens, overriding the \
             `launch_display` config (default [\"secondary\"]: move to the first non-main \
             display when more than one is attached, else leave in place). A selector or \
             ordered list: none|off, main|primary, secondary, index:N, id:N, uuid:X. Only \
             windows that did not exist before the call are moved, so an already-open \
             user window is never relocated. The response reports the move as `placement`.\n\n\
             Returns the launched app's pid, bundle_id, name, and a `windows` array \
             (same shape as `list_windows`) so callers can skip an extra round-trip before \
             `get_window_state(pid, window_id)`. When the focus-steal belt-and-braces \
             demotion check ran (target pid ≠ prior frontmost), the response also includes \
             `self_activation_suppressed: bool` — true if focus stayed with the prior \
             frontmost, false if the launched app held focus despite the re-demote attempt."
            .into(),
        input_schema: serde_json::json!({
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
                },
                "display": {
                    "type": ["string", "array"],
                    "items": { "type": "string" },
                    "description": "Display for windows this call opens; overrides the launch_display config. Selector or ordered preference list: none|off, main|primary, secondary, index:N, id:N, uuid:X. See get_screen_size `displays`."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: true,
    })
}

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cu_core::tool_args::ArgsExt;
        let bundle_id = args.opt_str("bundle_id");
        let name = args.opt_str("name");
        let mut urls: Vec<String> = args.str_array("urls");
        let cdp_debugging_port = args.opt_u64("cdp_debugging_port").map(|v| v as u16);
        let webkit_inspector_port = args.opt_u64("webkit_inspector_port").map(|v| v as u16);
        let creates_new_instance = args.bool_or("creates_new_application_instance", false);
        let mut additional_arguments: Vec<String> = args.str_array("additional_arguments");
        let display_policy = match args.get("display") {
            Some(raw) => match cu_core::display_placement::DisplayPolicy::from_json(raw) {
                Ok(p) => p,
                Err(e) => return ToolResult::error(format!("`display`: {e}")),
            },
            // Read fresh like get_desktop_state: `brigadier-cu config set` may
            // have written the file from another process.
            None => super::load_driver_config().launch_display,
        };
        // Windows that exist before the launch are never moved: they may be
        // the user's own windows of an already-running app.
        let preexisting_windows: std::collections::HashSet<u32> = crate::windows::all_windows()
            .into_iter()
            .map(|w| w.window_id)
            .collect();
        let preexisting_pids: std::collections::HashSet<i32> = crate::apps::list_running_apps()
            .into_iter()
            .map(|a| a.pid)
            .collect();
        // Re-launching a running app without open targets usually creates no
        // window, so placement should not wait for one.
        let expects_new_window = !urls.is_empty();

        if bundle_id.is_none() && name.is_none() {
            return ToolResult::error(
                "Provide either bundle_id or name to identify the app to launch.",
            );
        }
        if let Some(ref bid) = bundle_id {
            if crate::apps::resolve_bundle_id_to_locator(bid).is_none() {
                return structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for bundle_id '{bid}'."),
                    serde_json::json!({ "bundle_id": bid }),
                );
            }
        } else if let Some(ref n) = name {
            if crate::apps::locate_by_name(n).is_none() {
                return structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for name '{n}'."),
                    serde_json::json!({ "name": n }),
                );
            }
        }
        // A running app keeps the arguments it started with, so a file or link
        // given among them would be dropped without a word: open it as a url.
        let mut moved_targets = Vec::new();
        if !creates_new_instance && !additional_arguments.is_empty() {
            let running =
                crate::apps::list_running_apps()
                    .into_iter()
                    .any(|a| match bundle_id.as_deref() {
                        Some(bid) => a.bundle_id.as_deref() == Some(bid),
                        None => name.as_deref() == Some(a.name.as_str()),
                    });
            if running {
                let (targets, rest) = split_open_targets(additional_arguments);
                additional_arguments = rest;
                urls.extend(targets.iter().cloned());
                moved_targets = targets;
            }
        }
        if let Some(err) = preflight_file_urls(&urls) {
            return err;
        }

        // Build additional arguments (e.g., CDP port).
        if let Some(port) = cdp_debugging_port {
            additional_arguments.push(format!("--remote-debugging-port={port}"));
        }

        // Build env dict for webkit inspector.
        let mut env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        if let Some(port) = webkit_inspector_port {
            env.insert(
                "WEBKIT_INSPECTOR_SERVER".to_string(),
                format!("127.0.0.1:{port}"),
            );
            env.insert("TAURI_WEBVIEW_AUTOMATION".to_string(), "1".to_string());
        }

        let port_summary = {
            let mut s = String::new();
            if let Some(port) = cdp_debugging_port {
                s.push_str(&format!("\nCDP renderer available on port {port}."));
            }
            if let Some(port) = webkit_inspector_port {
                s.push_str(&format!("\nWebKit inspector available on port {port}."));
            }
            s
        };

        // ── Layer-3 focus-steal suppression (3-phase wrap) ───────────────
        //
        // Captures the prior frontmost pid, arms a wildcard suppression
        // BEFORE the launch (covers self-activations the target fires
        // synchronously during `open()`), then upgrades to a targeted
        // suppression keyed to the actual launched pid. Briefly holds
        // BOTH leases so a self-activation arriving in the wildcard→
        // targeted gap is still caught — that race is what hoang17's
        // Swift PR #1521 explicitly fixes; we do not regress it here.
        //
        // After 500ms (enough for `applicationDidFinishLaunching` +
        // any reflex `NSApp.activate(...)` to fire and get suppressed)
        // both leases are dropped. The belt-and-braces step at the end
        // re-activates the prior frontmost if the target is still
        // frontmost — handles the intra-`open()` synchronous activation
        // that fired before we could arm with the real pid.
        //
        // Explicit "watchable" mode is the exception: a host may request that
        // launched apps remain visible in front. Embedding alone preserves the
        // background-launch contract. With watchable fronting enabled we skip
        // suppression entirely and front the launched app after it starts.
        let watchable_front = cu_core::watchable_front_mode();
        let prior_frontmost = if suppresses_activation(watchable_front) {
            crate::apps::frontmost_pid()
        } else {
            None
        };

        let action_started = std::time::SystemTime::now();
        let wildcard_lease = prior_frontmost.map(|prior| {
            crate::focus_steal::FocusStealPreventer::begin_suppression(
                None,
                prior,
                "LaunchAppTool.pre",
            )
        });

        // A web browser that is already running is asked to open the links
        // itself, in a new window it keeps in the background, instead of
        // through LaunchServices, which brings it to the front. The
        // suppression armed above stays as a safety net.
        let opened_in_background = if creates_new_instance {
            None
        } else {
            crate::browser::open_link::open_in_running_browser(
                bundle_id.as_deref(),
                name.as_deref(),
                &urls,
                cdp_debugging_port,
            )
            .await
        };
        let link_count = urls.len();
        let windows_before_open = preexisting_windows.clone();
        let windows_before_summary = preexisting_windows.clone();

        // Predicate captured BEFORE moving inputs into spawn_blocking.
        // Same condition that selects the `openURLs:withApplicationAtURL:`
        // chain over the simpler `openApplicationAtURL:` path. Used after
        // the spawn returns to size the suppression window — the slow
        // path triggers a SECOND activation when the file-open delivers,
        // which lands AFTER the bundle-only-launch activation window.
        // Links the browser opened itself deliver no such second activation.
        let slow_launch_path = opened_in_background.is_none()
            && (!urls.is_empty()
                || !additional_arguments.is_empty()
                || !env.is_empty()
                || creates_new_instance);

        // Move the launch closure inputs into spawn_blocking. The
        // blocking task returns (pid, app_info, windows). Suppression
        // upgrade happens AFTER the blocking call returns (back on the
        // async runtime), then we sleep holding the targeted lease.
        let launch_result = tokio::task::spawn_blocking(move || {
            if let Some(opened) = opened_in_background {
                let pid = opened.pid;
                let (mut windows, new) = crate::browser::open_link::wait_for_new_windows(
                    pid,
                    &windows_before_open,
                    urls.len(),
                );
                if new == 0 {
                    main_window_first(pid, &mut windows);
                }
                crate::ax::offspace::remember_front_windows(pid);
                let app_info = crate::apps::list_running_apps()
                    .into_iter()
                    .find(|a| a.pid == pid);
                return Ok((pid, app_info, windows));
            }
            // An app already running opens the documents in windows of its
            // own; watch it first so each one is caught as it opens, also
            // when other documents open right after it.
            let mut opens_in_running_app = false;
            if !urls.is_empty() {
                let running =
                    crate::apps::list_running_apps().into_iter().find(|a| {
                        match bundle_id.as_deref() {
                            Some(bid) => a.bundle_id.as_deref() == Some(bid),
                            None => name.as_deref() == Some(a.name.as_str()),
                        }
                    });
                if let Some(app) = running {
                    crate::ax::window_watch::watch(app.pid);
                    opens_in_running_app = !creates_new_instance;
                }
            }
            let launch_once = || -> anyhow::Result<i32> {
                Ok(if let Some(ref bid) = bundle_id {
                    if urls.is_empty()
                        && additional_arguments.is_empty()
                        && env.is_empty()
                        && !creates_new_instance
                    {
                        crate::apps::launch_app(bid)?
                    } else {
                        crate::apps::launch_with_urls_by_bundle(
                            bid,
                            &urls,
                            &additional_arguments,
                            &env,
                            creates_new_instance,
                        )?
                    }
                } else {
                    let n = name.as_deref().unwrap();
                    if urls.is_empty()
                        && additional_arguments.is_empty()
                        && env.is_empty()
                        && !creates_new_instance
                    {
                        crate::apps::launch_app_by_name(n)?
                    } else {
                        crate::apps::launch_with_urls_by_name(
                            n,
                            &urls,
                            &additional_arguments,
                            &env,
                            creates_new_instance,
                        )?
                    }
                })
            };
            // An app that is still quitting (asked to quit just before) can't
            // be launched until it is gone: try again while it goes.
            let deadline = std::time::Instant::now() + QUITTING_WAIT;
            let mut retried = false;
            let pid = loop {
                match launch_once() {
                    Ok(pid) => break pid,
                    Err(error) => {
                        let quitting = crate::apps::list_running_apps().iter().any(|a| {
                            match bundle_id.as_deref() {
                                Some(bid) => a.bundle_id.as_deref() == Some(bid),
                                None => name.as_deref() == Some(a.name.as_str()),
                            }
                        });
                        if std::time::Instant::now() >= deadline || (retried && !quitting) {
                            return Err(error);
                        }
                        retried = true;
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            };

            // Retry loop: LaunchServices returns before WindowServer has
            // registered the new windows. Poll up to 5x100ms.
            // The documents open in new windows of the running app, often
            // only after LaunchServices returns: wait a little for them and
            // list them first, so the caller needn't list the windows again.
            let windows = if opens_in_running_app {
                let (mut windows, new) = crate::browser::open_link::wait_for_new_windows(
                    pid,
                    &windows_before_open,
                    urls.len(),
                );
                if new == 0 {
                    main_window_first(pid, &mut windows);
                }
                windows
            } else {
                let mut windows = resolve_windows_for_pid(pid);
                main_window_first(pid, &mut windows);
                windows
            };
            crate::ax::offspace::remember_front_windows(pid);

            let app_info: Option<crate::apps::AppInfo> = {
                let apps = crate::apps::list_running_apps();
                apps.into_iter().find(|a| a.pid == pid)
            };

            Ok::<_, anyhow::Error>((pid, app_info, windows))
        })
        .await;

        // Upgrade to targeted suppression now that we know the real pid.
        // Keep the wildcard lease alive until immediately AFTER we've
        // armed the targeted one — that's the PR #1521 overlap window.
        //
        // `self_activation_suppressed` is the outcome of the belt-and-
        // braces demotion check: `None` when the check didn't run
        // (no prior frontmost / launch failed / pid == prior), `Some(true)`
        // when the target was NOT frontmost after the suppression window
        // (or we successfully re-demoted it), `Some(false)` when the
        // re-demote failed and the target is still stealing focus.
        // Surfaced in the structured response so callers can observe
        // whether focus-steal prevention actually held.
        let mut self_activation_suppressed: Option<bool> = None;
        if let Ok(Ok((pid, _, _))) = &launch_result {
            if let Some(prior) = prior_frontmost {
                if *pid != prior {
                    // One lease covers both the call and the detached
                    // watchdog below, so there is no gap between them.
                    // An app this call started is always sent back, also
                    // when it comes forward while the user is typing.
                    let cold_start = !preexisting_pids.contains(pid);
                    let ticks = watchdog_ticks(slow_launch_path, cold_start);
                    let lasting = std::time::Duration::from_millis(250 * u64::from(ticks));
                    let targeted_lease =
                        crate::focus_steal::FocusStealPreventer::begin_suppression_for(
                            Some(*pid),
                            prior,
                            "LaunchAppTool.post",
                            lasting,
                            cold_start.then_some(action_started),
                        );
                    // Now safe to drop the wildcard — targeted is armed.
                    drop(wildcard_lease);
                    // Hold the targeted lease long enough to cover the
                    // ENTIRE post-launch activation window.
                    //
                    // - Fast path (bundle-only launch, no urls/args/env):
                    //   500ms covers `applicationDidFinishLaunching` plus
                    //   any reflex `NSApp.activate(...)`. Matches Swift
                    //   LaunchAppTool.swift exactly.
                    //
                    // - Slow path (urls / additional_arguments / env /
                    //   creates_new_instance): 2500ms. The slow-path
                    //   `openURLs:withApplicationAtURL:` chain triggers a
                    //   second activation when the file-open delivers to
                    //   the just-launched app — Electron apps (VSCode,
                    //   Cursor, Slack) re-`app.focus()` from inside their
                    //   `open-file` JS handler, AFTER our 500ms window
                    //   would have already closed. Empirically VSCode's
                    //   late activation can land anywhere from ~700ms to
                    //   ~2000ms after the openURLs return. The observer-
                    //   based lease catches any activation that lands
                    //   WHILE held, so widening the window converts the
                    //   late activation from a contract violation into
                    //   another auto-demote.
                    //
                    // Only the first moments are spent inside the call now:
                    // the detached watchdog below holds the same observer-
                    // backed lease for the rest of that window, so a late
                    // activation is still demoted while the agent already
                    // gets its answer.
                    let window_ms: u64 = 60;
                    tokio::time::sleep(std::time::Duration::from_millis(window_ms)).await;

                    // Belt-and-braces: if the target is in front anyway (an
                    // activation the notification missed), send it straight back
                    // through the WindowServer (back within ~50 ms, where an
                    // AppKit activation took about a second).
                    let mut demotion_succeeded = true;
                    if crate::apps::frontmost_pid() == Some(*pid) {
                        let sent_back = crate::input::skylight::set_front_pid(prior)
                            || crate::apps::activate_pid(prior);
                        demotion_succeeded = crate::apps::frontmost_pid() != Some(*pid);
                        if !demotion_succeeded {
                            tracing::warn!(
                                target: "platform_macos::tools::launch_app",
                                launched_pid = *pid,
                                prior_pid = prior,
                                sent_back,
                                "launched app remained frontmost after sending it back"
                            );
                        }
                    }
                    // Final in-call state determines the structured response.
                    let final_frontmost = crate::apps::frontmost_pid();
                    self_activation_suppressed =
                        Some(final_frontmost != Some(*pid) && demotion_succeeded);

                    // Detached late-activation watchdog (slow path only).
                    //
                    // Why: Electron apps with no workspace open (cold-
                    // launched VSCode / Cursor / Slack with a file URL)
                    // re-activate AGAIN when their Welcome window
                    // finishes loading — empirically 4-8 seconds after
                    // the `openURLs:withApplicationAtURL:` call returns.
                    // That's well past the in-call suppression window
                    // and any reasonable extension of it that an agent
                    // workflow would tolerate as caller latency.
                    //
                    // Solution: hold a fresh observer-backed lease in
                    // the background for ~8s, demoting if the launched
                    // pid pops back. The caller doesn't wait — the tool
                    // already returned its honest `self_activation_
                    // suppressed` for the in-call window. The detached
                    // task just keeps the no-foreground-steal contract
                    // honored past the RPC boundary.
                    //
                    // Note on process lifecycle: this watchdog only runs
                    // when the tokio runtime stays alive — i.e. in the
                    // long-running `brigadier-cu mcp` / `brigadier-cu serve`
                    // daemon modes. The one-shot `brigadier-cu call` mode
                    // exits as soon as the tool returns, taking the
                    // detached task with it. Acceptable because the
                    // contract is "no foreground steal during a session
                    // the agent is driving" — `brigadier-cu call` doesn't
                    // have a session that outlives the call.
                    //
                    // Tradeoffs:
                    // - Caller latency: ~0.1 s.
                    // - Total coverage: ~10.5 s post-launch (slow path or a
                    //   cold start), 0.5 s otherwise.
                    // - CPU: none while waiting. The lease is answered from
                    //   the activation notification itself, which sends the
                    //   app back at once and leaves alone a switch the user
                    //   made with their own keyboard or pointer, so there is
                    //   no polling and no second, slower demote.
                    tokio::spawn(async move {
                        tokio::time::sleep(
                            lasting.saturating_sub(std::time::Duration::from_millis(window_ms)),
                        )
                        .await;
                        drop(targeted_lease);
                    });
                } else {
                    // pid == prior frontmost (re-launch of an already-
                    // frontmost app). Just drop the wildcard.
                    drop(wildcard_lease);
                }
            }
        } else {
            // Launch failed; just drop the lease.
            drop(wildcard_lease);
        }

        // Explicit "watchable" fronting: bring the just-launched app forward so
        // the user watching the agent sees the app they asked to open. Best-
        // effort — a rejected activation never fails the launch. Only runs when
        // opted in (otherwise suppression above kept it in the background).
        if watchable_front {
            if let Ok(Ok((pid, _, _))) = &launch_result {
                let activated = crate::apps::activate_pid_all_windows(*pid);
                if !activated {
                    tracing::warn!(
                        target: "platform_macos::tools::launch_app",
                        launched_pid = *pid,
                        "watchable launch: activation returned NO — \
                         launched app may not have come to the foreground"
                    );
                }
            }
        }

        // Display placement runs after the activation window so windows that
        // appear late (Calculator's shows ~1s after launch) are still new
        // windows we can see and move.
        let launch_result = match launch_result {
            Ok(Ok((pid, app_info, mut windows))) => {
                let wait = if preexisting_pids.contains(&pid) && !expects_new_window {
                    std::time::Duration::ZERO
                } else {
                    crate::displays::PLACEMENT_WINDOW_WAIT
                };
                let placed = tokio::task::spawn_blocking(move || {
                    let p = crate::displays::place_new_windows(
                        pid,
                        &mut windows,
                        &preexisting_windows,
                        &display_policy,
                        wait,
                    );
                    (windows, p)
                })
                .await;
                match placed {
                    Ok((windows, p)) => Ok(Ok((pid, app_info, windows, p))),
                    Err(e) => Err(e),
                }
            }
            Ok(Err(e)) => Ok(Err(e)),
            Err(e) => Err(e),
        };

        match launch_result {
            Ok(Ok((pid, app_info, windows, display_placement))) => {
                let app_name = app_info.as_ref().map(|a| a.name.as_str()).unwrap_or("?");
                let bid = app_info
                    .as_ref()
                    .and_then(|a| a.bundle_id.as_deref())
                    .unwrap_or("?");

                let placement = if watchable_front {
                    "foreground"
                } else {
                    "background"
                };
                let mut summary = match opened_in_background {
                    Some(opened) => {
                        let links = if link_count == 1 {
                            "the link".to_owned()
                        } else {
                            format!("{link_count} links")
                        };
                        let how = match opened.by {
                            OpenedBy::DevTools(port) => {
                                format!("through its debugging port {port}")
                            }
                            OpenedBy::AppleEvents => "by Apple Events".to_owned(),
                        };
                        let mut s = format!(
                            "{app_name} (pid {pid}) was already running: opened {links} in a new \
                             background window, {how}."
                        );
                        if windows
                            .first()
                            .is_some_and(|w| !windows_before_summary.contains(&w.window_id))
                        {
                            s.push_str(" The new window is listed first.");
                        }
                        s
                    }
                    None => {
                        format!("Launched {app_name} (pid {pid}) in {placement}.{port_summary}")
                    }
                };

                if !windows.is_empty() {
                    summary.push_str("\n\nWindows:");
                    for w in &windows {
                        let title = if w.title.is_empty() {
                            "(no title)".to_owned()
                        } else {
                            format!("\"{}\"", w.title)
                        };
                        summary.push_str(&format!("\n- {title} [window_id: {}]", w.window_id));
                    }
                    summary.push_str(&format!(
                        "\n→ Call get_window_state(pid: {pid}, window_id) to inspect."
                    ));
                }

                let windows_json: Vec<Value> = windows
                    .iter()
                    .map(|w| {
                        serde_json::json!({
                            "window_id": w.window_id,
                            "pid": w.pid,
                            "app_name": w.app_name,
                            "title": w.title,
                            "bounds": {
                                "x": w.bounds.x, "y": w.bounds.y,
                                "width": w.bounds.width, "height": w.bounds.height
                            },
                            "is_on_screen": w.is_on_screen,
                        })
                    })
                    .collect();

                let mut structured = serde_json::json!({
                    "pid": pid,
                    "bundle_id": bid,
                    "name": app_name,
                    "windows": windows_json,
                });
                // Only emit `self_activation_suppressed` when the
                // belt-and-braces demotion check actually ran. `None`
                // means the launch didn't enter the focus-steal path
                // (no prior frontmost, or pid == prior) — surfacing
                // a stale `false` would be misleading.
                if let Some(suppressed) = self_activation_suppressed {
                    structured["self_activation_suppressed"] = serde_json::Value::Bool(suppressed);
                }
                if let Some(p) = display_placement {
                    let moved = p["moved"].as_array().map_or(0, Vec::len);
                    if moved > 0 {
                        summary.push_str(&format!(
                            "\nMoved {moved} new window(s) to display {}.",
                            p["display_id"]
                        ));
                    }
                    structured["placement"] = p;
                }
                if let Some(opened) = opened_in_background {
                    structured["opened_in_background"] = match opened.by {
                        OpenedBy::DevTools(port) => {
                            serde_json::json!({ "by": "debugging_port", "port": port })
                        }
                        OpenedBy::AppleEvents => serde_json::json!({ "by": "apple_events" }),
                    };
                }
                if !moved_targets.is_empty() {
                    summary.push_str(&format!(
                        "\nThe app was already running, so {} from additional_arguments \
                         opened as urls.",
                        moved_targets.join(", ")
                    ));
                    structured["opened_from_arguments"] = serde_json::json!(moved_targets);
                }
                ToolResult::text(summary).with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::error(format!("Launch failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// How many 250 ms ticks the detached watchdog lease keeps a launched app from
/// coming to the front. An app that starts from scratch activates itself
/// once its first window is ready, seconds after the launch returns
/// (Electron apps especially), as does one that is handed documents, so
/// both get ~10.5 s; an app that was already running only reacts at once.
fn watchdog_ticks(slow_launch_path: bool, cold_start: bool) -> u32 {
    if slow_launch_path || cold_start {
        42
    } else {
        2
    }
}

/// Whether `launch_app` should arm background-launch focus-steal suppression.
///
/// Ordinary embedded/serve/daemon/one-shot use: `true` — keep the launched app
/// in the background and restore the prior frontmost. Explicit watchable mode:
/// `false` — no suppression, because the host asked to leave the app in front.
fn suppresses_activation(watchable_front: bool) -> bool {
    !watchable_front
}

// ── Blocking helpers ──────────────────────────────────────────────────────────

/// Poll for the pid's layer-0 windows, retrying up to 5x100ms to absorb
/// LaunchServices → WindowServer latency (mirrors the Swift reference).
/// How long a launch waits for an earlier instance of the app to finish
/// quitting.
const QUITTING_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Lists the app's main window first: the one the launch opened or brought
/// up, ahead of any panels or helper windows the app keeps.
fn main_window_first(pid: i32, windows: &mut [crate::windows::WindowInfo]) {
    use crate::ax::bindings::{ax_get_window_id, copy_element_attr, AXUIElementCreateApplication};
    let main = unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return;
        }
        let window = copy_element_attr(app, "AXMainWindow");
        core_foundation::base::CFRelease(app as _);
        let Some(window) = window else {
            return;
        };
        let id = ax_get_window_id(window);
        core_foundation::base::CFRelease(window as _);
        id
    };
    if let Some(at) = windows.iter().position(|w| Some(w.window_id) == main) {
        windows[..=at].rotate_right(1);
    }
}

fn resolve_windows_for_pid(pid: i32) -> Vec<crate::windows::WindowInfo> {
    for attempt in 0..5 {
        let found = crate::displays::current_windows_for_pid(pid);
        if !found.is_empty() {
            return found;
        }
        if attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    vec![]
}

fn structured_launch_error(code: &str, message: String, details: serde_json::Value) -> ToolResult {
    let mut payload = serde_json::json!({
        "error": code,
    });

    match details {
        serde_json::Value::Object(details) => {
            if let serde_json::Value::Object(payload) = &mut payload {
                payload.extend(details);
            }
        }
        details => {
            if let serde_json::Value::Object(payload) = &mut payload {
                payload.insert("details".to_string(), details);
            }
        }
    }

    ToolResult::error(message).with_structured(payload)
}

/// Splits launch arguments into files and links to open (a link, or an absolute or `~` path)
/// and the rest (flags and their values).
fn split_open_targets(arguments: Vec<String>) -> (Vec<String>, Vec<String>) {
    arguments.into_iter().partition(|argument| {
        !argument.starts_with('-')
            && (argument.contains("://") || argument.starts_with('/') || argument.starts_with('~'))
    })
}

fn preflight_file_urls(urls: &[String]) -> Option<ToolResult> {
    for raw in urls {
        let Some(path) = local_file_target(raw) else {
            continue;
        };
        if !path.exists() {
            return Some(structured_launch_error(
                "FILE_NOT_FOUND",
                format!(
                    "Local launch_app url target does not exist: {}",
                    path.display()
                ),
                serde_json::json!({
                    "url": raw,
                    "path": path.display().to_string(),
                }),
            ));
        }
    }
    None
}

fn local_file_target(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() {
        return Some(PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        // A page anchor or query isn't part of the file's path.
        let path = path.split(['#', '?']).next().unwrap_or(path);
        let decoded = percent_decode_path(path);
        return Some(expand_tilde(&decoded));
    }
    let looks_like_url = raw.contains(':') && !raw.starts_with('/') && !raw.starts_with('~');
    if looks_like_url {
        return None;
    }
    Some(expand_tilde(raw))
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn percent_decode_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                decoded.push((high << 4) | low);
                i += 3;
                continue;
            }
        }

        decoded.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {

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
    use super::{
        local_file_target, preflight_file_urls, split_open_targets, suppresses_activation,
        watchdog_ticks,
    };
    use std::path::PathBuf;

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
    fn ordinary_launch_still_suppresses_activation() {
        // Embedding alone, serve/daemon, and one-shot CLI all keep background
        // launch + prior-frontmost restoration.
        assert!(
            suppresses_activation(false),
            "launch_app must keep suppression without explicit watchable opt-in"
        );
    }

    #[test]
    fn watchable_launch_does_not_suppress_activation() {
        // Explicit watchable mode fronts the launched app instead of hiding it.
        assert!(
            !suppresses_activation(true),
            "watchable launch_app must front the launched app (no suppression)"
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
}
