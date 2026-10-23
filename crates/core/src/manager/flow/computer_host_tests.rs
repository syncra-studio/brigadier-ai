//! Computer use as Settings and the watchdog tend it, against a fake helper: a script in the
//! helper's place that answers `host` requests from a file and notes each one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Flow, Options, Reply};
use crate::{ComputerSetup, PermissionState};

/// The fake helper, placed where the real one runs from, answering `status` with `status`.
fn fake_helper(flow: &Flow, status: &str) -> PathBuf {
    let home = flow.manager.data_dir.join("computer");
    let bin = home
        .join(super::super::computer_use::HELPER_APP)
        .join("Contents/MacOS");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join(super::super::computer_use::HELPER_BIN);
    std::fs::write(
        &script,
        r#"#!/bin/sh
shift
echo "$*" >> "$BRIGADIER_CU_HOME/calls.log"
case "$1" in
  status) cat "$BRIGADIER_CU_HOME/fake-status" ;;
  restart|ensure|probe) echo '{"state":"healthy","pid":1}' ;;
  off) touch "$BRIGADIER_CU_HOME/off"; echo '{}' ;;
  on) rm -f "$BRIGADIER_CU_HOME/off"; echo '{}' ;;
  *) echo '{}' ;;
esac
"#,
    )
    .unwrap();
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&script)
        .status()
        .unwrap();
    answer(&home, status);
    home
}

fn answer(home: &Path, status: &str) {
    std::fs::write(home.join("fake-status"), status).unwrap();
}

/// A live helper process on record: this test process stands in for it.
fn record_helper(home: &Path) {
    std::fs::write(home.join("cu.pid"), std::process::id().to_string()).unwrap();
}

fn calls(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

const HEALTHY: &str = r#"{"state":"healthy","permissions":{"accessibility":true,"screen_recording":true},"sessions":[{"session":"brigadier-someone","apps":[{"name":"TextEdit","bundle_id":"com.apple.TextEdit","pid":42}]}]}"#;
const HUNG: &str = r#"{"state":"hung"}"#;

async fn flow(name: &str) -> Flow {
    Flow::start(
        name,
        Options::default(),
        std::sync::Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await
}

#[tokio::test]
async fn showing_computer_use_starts_nothing_when_no_helper_runs() {
    let flow = flow("cu-host-idle").await;
    let home = fake_helper(&flow, HEALTHY);
    let status = flow.manager.computer_use_status().await;
    assert_eq!(status.setup, ComputerSetup::NotSetUp);
    assert_eq!(status.accessibility, PermissionState::Unknown);
    assert!(!status.current);
    flow.manager.watch_computer_helper(false).await;
    flow.manager.watch_computer_helper(true).await;
    assert!(calls(&home).is_empty(), "{:?}", calls(&home));
}

#[tokio::test]
async fn what_the_helper_last_wrote_shows_as_not_current() {
    let flow = flow("cu-host-cached").await;
    let home = fake_helper(&flow, HEALTHY);
    std::fs::write(
        home.join("permissions.json"),
        r#"{"accessibility":true,"screen_recording":false,"pid":9,"build_id":"x"}"#,
    )
    .unwrap();
    let status = flow.manager.computer_use_status().await;
    assert!(!status.current);
    assert_eq!(status.accessibility, PermissionState::Granted);
    assert_eq!(status.screen_recording, PermissionState::NotGranted);
    assert_eq!(status.setup, ComputerSetup::Missing);
    assert!(status.sessions.is_empty());
    assert!(calls(&home).is_empty());
}

#[tokio::test]
async fn finished_setup_shows_as_set_up_while_no_helper_has_read_the_permissions() {
    let flow = flow("cu-host-set-up").await;
    let home = fake_helper(&flow, HEALTHY);
    std::fs::write(
        home.join(super::super::computer_host::CAPTURE_CONFIRMED),
        b"",
    )
    .unwrap();
    let status = flow.manager.computer_use_status().await;
    assert!(!status.current);
    assert_eq!(status.setup, ComputerSetup::Ready);
    assert!(calls(&home).is_empty());
}

#[tokio::test]
async fn a_running_helper_shows_its_permissions_and_the_agents_using_apps() {
    let flow = flow("cu-host-live").await;
    let home = fake_helper(&flow, HEALTHY);
    record_helper(&home);
    std::fs::write(
        home.join(super::super::computer_host::CAPTURE_CONFIRMED),
        b"",
    )
    .unwrap();
    let status = flow.manager.computer_use_status().await;
    assert!(status.current);
    assert_eq!(status.setup, ComputerSetup::Ready);
    assert_eq!(status.sessions.len(), 1);
    assert_eq!(status.sessions[0].app, "TextEdit");
    assert_eq!(status.sessions[0].bundle_id, "com.apple.TextEdit");
    assert_eq!(status.sessions[0].label, "An agent");
    // The watchdog tells the windows, and a window that subscribes later hears it too.
    assert!(flow.manager.last_computer_status().is_none());
    flow.manager.watch_computer_helper(false).await;
    assert_eq!(flow.manager.last_computer_status(), Some(status));
}

#[tokio::test]
async fn a_helper_that_stops_answering_is_restarted_on_the_second_look() {
    let flow = flow("cu-host-hung").await;
    let home = fake_helper(&flow, HUNG);
    record_helper(&home);
    flow.manager.watch_computer_helper(false).await;
    assert!(!calls(&home).contains(&"restart".to_owned()));
    flow.manager.watch_computer_helper(false).await;
    assert_eq!(
        calls(&home).iter().filter(|c| *c == "restart").count(),
        1,
        "{:?}",
        calls(&home)
    );
    // It answers again: no more restarts.
    answer(&home, HEALTHY);
    flow.manager.watch_computer_helper(false).await;
    flow.manager.watch_computer_helper(false).await;
    assert_eq!(calls(&home).iter().filter(|c| *c == "restart").count(), 1);
    assert!(!matches!(
        flow.manager.computer_use_status().await.setup,
        ComputerSetup::Restarting | ComputerSetup::Failed { .. }
    ));
}

#[tokio::test]
async fn right_after_a_wake_one_unanswered_look_is_enough() {
    let flow = flow("cu-host-wake").await;
    let home = fake_helper(&flow, HUNG);
    record_helper(&home);
    flow.manager.watch_computer_helper(true).await;
    assert_eq!(calls(&home).iter().filter(|c| *c == "restart").count(), 1);
}

#[tokio::test]
async fn nothing_is_tended_while_computer_use_is_off() {
    let flow = flow("cu-host-off").await;
    let home = fake_helper(&flow, HUNG);
    record_helper(&home);
    let mut settings = flow.core.settings();
    settings.computer_use = false;
    flow.core.update_settings(settings).await.unwrap();
    flow.manager.watch_computer_helper(true).await;
    flow.manager.watch_computer_helper(false).await;
    // The helper's off mark is set, once; nothing is started or restarted.
    let calls = calls(&home);
    assert_eq!(
        calls.iter().filter(|call| *call == "off").count(),
        1,
        "{calls:?}"
    );
    assert!(
        calls.iter().all(|call| call == "off" || call == "status"),
        "{calls:?}"
    );
    assert_eq!(
        flow.manager.computer_use_status().await.setup,
        ComputerSetup::Off
    );
}

#[tokio::test]
async fn quick_flips_of_computer_use_end_as_the_last_one() {
    let flow = flow("cu-host-flips").await;
    let home = fake_helper(&flow, HEALTHY);
    let mut settings = flow.core.settings();
    settings.computer_use = false;
    flow.core.update_settings(settings).await.unwrap();
    // Off, then on, then off, each turn started before the last one ran.
    let turns: Vec<_> = (0..3)
        .map(|_| {
            let manager = flow.manager.clone();
            tokio::spawn(async move { manager.computer_use_turned().await })
        })
        .collect();
    for turn in turns {
        turn.await.unwrap();
    }
    assert!(home.join("off").exists(), "{:?}", calls(&home));
    assert!(
        calls(&home).iter().all(|call| call == "off"),
        "{:?}",
        calls(&home)
    );
}

#[tokio::test]
async fn checks_at_the_same_time_refresh_the_helper_once() {
    let flow = flow("cu-host-check").await;
    let home = fake_helper(
        &flow,
        r#"{"state":"healthy","permissions":{"accessibility":true,"screen_recording":false}}"#,
    );
    record_helper(&home);
    let (a, b, c) = tokio::join!(
        flow.manager.check_computer_use(),
        flow.manager.check_computer_use(),
        flow.manager.check_computer_use(),
    );
    for status in [a, b, c] {
        assert_eq!(status.unwrap().setup, ComputerSetup::Missing);
    }
    let refreshes = calls(&home)
        .iter()
        .filter(|c| c.starts_with("request permissions_refresh"))
        .count();
    assert_eq!(refreshes, 1, "{:?}", calls(&home));
    // With both granted, a check only reads.
    answer(&home, HEALTHY);
    tokio::time::sleep(Duration::from_millis(10)).await;
    flow.manager.check_computer_use().await.unwrap();
    assert_eq!(
        calls(&home)
            .iter()
            .filter(|c| c.starts_with("request permissions_refresh"))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_agent_waiting_for_setup_opens_the_setup_window_once() {
    let flow = flow("cu-host-setup").await;
    let home = fake_helper(&flow, HEALTHY);
    let mut news = flow.manager.subscribe_computer_use();
    // Asked before Brigadier started: old news.
    flow.manager.watch_computer_helper(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    std::fs::write(home.join("setup-needed"), "1").unwrap();
    flow.manager.watch_computer_helper(false).await;
    flow.manager.watch_computer_helper(false).await;
    let mut asked = 0;
    while let Ok(item) = news.try_recv() {
        if matches!(item, crate::manager::ComputerNews::SetupNeeded) {
            asked += 1;
        }
    }
    assert_eq!(asked, 1);
}

/// A helper bundled where Brigadier.app keeps it, signed (ad hoc) so it passes the signature
/// check when placed: computer use is then really available to agents.
pub(super) fn bundle_helper(flow: &Flow) {
    let app = flow
        .manager
        .config
        .daemon_exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("Helpers")
        .join(super::super::computer_use::HELPER_APP);
    let bin = app.join("Contents/MacOS");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(
        "/usr/bin/true",
        bin.join(super::super::computer_use::HELPER_BIN),
    )
    .unwrap();
    std::fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>ai.brigadier.test-computer-use</string><key>CFBundleExecutable</key><string>brigadier-cu</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>
"#,
    )
    .unwrap();
    let signed = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app)
        .output()
        .unwrap();
    assert!(signed.status.success(), "{signed:?}");
}
