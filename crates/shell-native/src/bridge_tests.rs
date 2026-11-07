//! Exercises the desktop connection without starting a daemon or showing system notices.
use super::*;
use brigadier_core::{AccountsView, Catalog};
use brigadier_ipc::{Accepted, Listener, Token};

#[tokio::test]
async fn preconnection_quota_warning_notifies_once_across_connections_and_restart() {
    quota_recovery(true).await;
}

#[tokio::test]
async fn quota_warning_restored_after_startup_checks_notifies_once() {
    quota_recovery(false).await;
}

async fn quota_recovery(initially_ready: bool) {
    let dir = std::env::temp_dir().join(format!(
        "20261018-1945-issues-run-2-w10-quota-{}-{initially_ready}",
        std::process::id()
    ));
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(dir.clone()),
    })
    .unwrap();
    let token = Token::generate().unwrap();
    platform
        .private_fs()
        .create_private_dir(&platform.paths().run_dir)
        .unwrap();
    token
        .publish(&*platform, &platform.paths().token_path)
        .unwrap();
    let listener = Listener::bind(&*platform).unwrap();
    let warning: QuotaWarning = serde_json::from_value(serde_json::json!({
        "account": {"provider": "claude", "account": "work"},
        "who": "Work (Claude Code)", "window": "seven_day", "leftPercent": 35.4,
        "resetsAtMs": (brigadier_core::now_ms() / 3_600_000 + 9) * 3_600_000 - 1
    }))
    .unwrap();
    let live = warning.clone();
    let (snapshotted, mut snapshots) = mpsc::unbounded_channel();
    let (disconnect, mut disconnects) = mpsc::unbounded_channel::<()>();
    let (subscribed, mut subscriptions) = mpsc::unbounded_channel();
    let mut server = tokio::spawn(async move {
        let mut connections = 0;
        loop {
            connections += 1;
            let head = if initially_ready || connections == 1 {
                42
            } else {
                43
            };
            let Accepted::Client { mut connection, .. } = listener
                .accept()
                .await
                .unwrap()
                .handshake(&token)
                .await
                .unwrap()
            else {
                panic!("expected desktop client")
            };
            connection
                .writer
                .write(&ServerFrame::Welcome {
                    daemon: DaemonInfo {
                        pid: 1,
                        version: "test".into(),
                        protocol: brigadier_ipc::protocol::PROTOCOL_VERSION,
                        platform: "test".into(),
                        started_at_ms: 0,
                        data_dir: String::new(),
                    },
                    last_seq: head,
                })
                .await
                .unwrap();
            loop {
                let frame = tokio::select! {
                    _ = disconnects.recv() => break,
                    frame = connection.reader.read::<ClientFrame>() => frame.unwrap(),
                };
                let Some(ClientFrame::Request { id, request }) = frame else {
                    break;
                };
                let mut refreshed = warning.clone();
                if connections > 1 {
                    // A provider refresh nudges the same reset across an hour boundary.
                    refreshed.resets_at_ms += 1_000;
                }
                let accounts = AccountsView {
                    warnings: vec![refreshed],
                    ..Default::default()
                };
                let restore_later =
                    matches!(request, Request::GetAccounts) && !initially_ready && connections == 1;
                let response = match request {
                    Request::Subscribe { after_seq, .. } => {
                        assert_eq!(after_seq, head);
                        subscribed.send(()).unwrap();
                        continue;
                    }
                    Request::GetAccounts => {
                        snapshotted.send(()).unwrap();
                        Response::GetAccounts {
                            accounts: if restore_later {
                                AccountsView::default()
                            } else {
                                accounts.clone()
                            },
                        }
                    }
                    Request::GetCatalog => Response::GetCatalog {
                        catalog: Box::new(Catalog {
                            projects: vec![],
                            conversations: vec![],
                            settings: Settings::default(),
                        }),
                    },
                    Request::Shutdown => Response::Shutdown,
                    _ => panic!("unexpected request: {request:?}"),
                };
                connection
                    .writer
                    .write(&ServerFrame::Response {
                        id,
                        result: Outcome::Ok {
                            value: Box::new(response),
                        },
                    })
                    .await
                    .unwrap();
                if restore_later {
                    connection
                        .writer
                        .write(&ServerFrame::Event {
                            event: EventEnvelope {
                                seq: 43,
                                stream: "accounts".into(),
                                stream_seq: 1,
                                at_ms: brigadier_core::now_ms(),
                                event: brigadier_ipc::protocol::RawJson(
                                    serde_json::value::to_raw_value(
                                        &DomainEvent::AccountsChecked { accounts },
                                    )
                                    .unwrap(),
                                ),
                            },
                        })
                        .await
                        .unwrap();
                }
            }
        }
    });
    let (shown, mut notices) = mpsc::unbounded_channel();
    let start = || {
        let shown = shown.clone();
        Bridge::start(
            platform.clone(),
            Launcher::new(platform.clone(), vec![]),
            Hooks {
                notify: Box::new(move |title, body| {
                    shown.send((title.to_owned(), body.to_owned())).unwrap();
                }),
                in_front: Box::new(|| false),
                open_setup: Box::new(|| {}),
                computer_use_changed: Box::new(|_| {}),
                is_quitting: Box::new(|| false),
            },
        )
    };
    let bridge = start();
    let first = tokio::time::timeout(Duration::from_secs(3), notices.recv()).await;
    if first.is_err() {
        bridge.inner.stopping.store(true, Ordering::Release);
        disconnect.send(()).unwrap();
        server.abort();
        let _ = (&mut server).await;
        std::fs::remove_dir_all(&dir).unwrap();
        if let Some(socket_dir) = platform.paths().socket_dir() {
            std::fs::remove_dir(socket_dir).unwrap();
        }
    }
    let (title, body) = first
        .expect("warning recorded before subscription must notify")
        .unwrap();
    assert_eq!(title, "Weekly quota resets soon");
    assert!(body.contains("35%"));
    subscriptions.recv().await.unwrap();
    snapshots.recv().await.unwrap();
    disconnect.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), subscriptions.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), snapshots.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), notices.recv())
            .await
            .is_err()
    );
    assert!(bridge.shutdown_daemon(Duration::from_secs(3)).await);
    disconnect.send(()).unwrap();
    let mut link = bridge.inner.link.subscribe();
    tokio::time::timeout(
        Duration::from_secs(3),
        link.wait_for(|state| *state == Link::Down),
    )
    .await
    .unwrap()
    .unwrap();
    drop(bridge);

    let restarted = start();
    tokio::time::timeout(Duration::from_secs(3), subscriptions.recv())
        .await
        .unwrap()
        .unwrap();
    // The snapshot and live event must use the same durable notification key.
    restarted.notify(&EventEnvelope {
        seq: 43,
        stream: "quota-warnings".into(),
        stream_seq: 1,
        at_ms: brigadier_core::now_ms(),
        event: brigadier_ipc::protocol::RawJson(
            serde_json::value::to_raw_value(&DomainEvent::QuotaWillBeLost { warning: live })
                .unwrap(),
        ),
    });
    tokio::time::timeout(Duration::from_secs(3), snapshots.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), notices.recv())
            .await
            .is_err()
    );
    assert!(restarted.shutdown_daemon(Duration::from_secs(3)).await);
    disconnect.send(()).unwrap();
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(&dir).unwrap();
    if let Some(socket_dir) = platform.paths().socket_dir() {
        std::fs::remove_dir(socket_dir).unwrap();
    }
}

#[test]
fn quota_notification_keys_survive_restart_but_allow_the_next_reset() {
    let dir = std::env::temp_dir().join(format!(
        "20261018-1945-issues-run-2-w10-keys-{}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("keys.json");
    let mut keys = NotificationKeys::load(path.clone());
    assert!(keys.insert("quota:claude:work:seven_day:5000".into(), 1000));
    drop(keys);
    let mut keys = NotificationKeys::load(path.clone());
    assert!(!keys.insert("quota:claude:work:seven_day:6000".into(), 2000));
    assert!(keys.insert("quota:claude:work:seven_day:15000".into(), 6001));
    assert!(keys.insert("quota:claude:personal:seven_day:15000".into(), 6001));
    assert!(keys.insert("finished:m1:5".into(), 6001));
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(!saved.contains("finished:"));
    std::fs::remove_dir_all(dir).unwrap();
}

/// A data folder of its own with a published token and a bound endpoint, standing in for a
/// daemon of another version.
struct OtherDaemon {
    dir: PathBuf,
    platform: Arc<dyn Platform>,
    token: Token,
    listener: Listener,
}

impl OtherDaemon {
    fn start(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "win-issues-w10-bridge-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.clone()),
        })
        .unwrap();
        platform
            .private_fs()
            .create_private_dir(&platform.paths().run_dir)
            .unwrap();
        let token = Token::generate().unwrap();
        token
            .publish(&*platform, &platform.paths().token_path)
            .unwrap();
        let listener = Listener::bind(&*platform).unwrap();
        Self {
            dir,
            platform,
            token,
            listener,
        }
    }

    /// The app's next hello, accepted.
    async fn next(&self) -> Connection {
        let pending = self.listener.accept().await.unwrap();
        let Accepted::Client { connection, .. } = pending.handshake(&self.token).await.unwrap()
        else {
            panic!("expected the app's hello");
        };
        connection
    }

    /// Takes every hello and drops it unanswered, as a daemon from before #771 did.
    async fn drop_hellos(&self) {
        loop {
            let pending = self.listener.accept().await.unwrap();
            drop(pending.handshake(&self.token).await);
        }
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        if let Some(dir) = self.platform.paths().socket_dir() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn daemon_info(protocol: u32, pid: u32) -> DaemonInfo {
    DaemonInfo {
        pid,
        version: "0.0.1-other".into(),
        protocol,
        platform: "test".into(),
        started_at_ms: 0,
        data_dir: String::new(),
    }
}

/// No process has this id.
const GONE_PID: u32 = u32::MAX - 2;

fn bridge_for(daemon: &OtherDaemon, launcher: Launcher, waits: Waits) -> Bridge {
    Bridge::start_with(
        daemon.platform.clone(),
        launcher,
        Hooks {
            notify: Box::new(|_, _| {}),
            in_front: Box::new(|| false),
            open_setup: Box::new(|| {}),
            computer_use_changed: Box::new(|_| {}),
            is_quitting: Box::new(|| false),
        },
        waits,
    )
}

fn quick_waits() -> Waits {
    Waits {
        quit_ack: Duration::from_millis(400),
        quit_exit: Duration::from_millis(400),
        blocked: Duration::from_millis(200),
    }
}

/// The reason the bridge gives once it says it is blocked.
async fn blocked_reason(bridge: &Bridge) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = bridge.inner.status.lock().unwrap().clone();
            if let Some(BridgeEvent::Blocked { reason }) = status {
                return reason;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the app never said it was blocked")
}

fn stop(bridge: Bridge) {
    bridge.inner.stopping.store(true, Ordering::Release);
}

/// The app's next request on `connection`; None once it closed.
async fn next_request(connection: &mut Connection) -> Option<(u32, Request)> {
    match connection.reader.read::<ClientFrame>().await.ok()?? {
        ClientFrame::Request { id, request } => Some((id, request)),
        other => panic!("unexpected frame: {other:?}"),
    }
}

/// An update landed while an older daemon ran (#771): the app is told its version, asks it to
/// quit as Quit does, waits for it to go, and connects to the one it launches. Before, the
/// hello went unanswered and the app relaunched forever.
#[tokio::test]
async fn an_older_daemon_is_asked_to_quit_and_the_app_connects_to_its_own() {
    let daemon = OtherDaemon::start("older");
    let launches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let launcher = Launcher::fake(daemon.platform.clone(), {
        let launches = launches.clone();
        move || {
            launches.fetch_add(1, Ordering::AcqRel);
            crate::launcher::Launched::running()
        }
    });
    let bridge = bridge_for(&daemon, launcher, Waits::default());
    let mut old = daemon.next().await;
    old.writer
        .write(&ServerFrame::ProtocolMismatch {
            daemon: daemon_info(PROTOCOL_VERSION - 1, GONE_PID),
        })
        .await
        .unwrap();
    let (id, request) = tokio::time::timeout(Duration::from_secs(5), next_request(&mut old))
        .await
        .unwrap()
        .expect("the older daemon was not asked to quit");
    assert!(matches!(request, Request::Shutdown));
    old.writer
        .write(&ServerFrame::Response {
            id,
            result: Outcome::Ok {
                value: Box::new(Response::Shutdown),
            },
        })
        .await
        .unwrap();
    drop(old);
    // Its own daemon, launched once the old one is gone, welcomes it.
    let mut own = tokio::time::timeout(Duration::from_secs(5), daemon.next())
        .await
        .unwrap();
    own.writer
        .write(&ServerFrame::Welcome {
            daemon: daemon_info(PROTOCOL_VERSION, std::process::id()),
            last_seq: 0,
        })
        .await
        .unwrap();
    let mut link = bridge.inner.link.subscribe();
    tokio::time::timeout(
        Duration::from_secs(5),
        link.wait_for(|link| *link == Link::Up),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(launches.load(Ordering::Acquire), 1);
    stop(bridge);
    drop(own);
    daemon.cleanup();
}

/// A daemon from before #771 drops the app's hello unanswered, and the daemon the app launches
/// exits at once, since that one holds the lock: the app says so and how to clear it, and
/// stops launching. Before, it relaunched every few seconds for as long as that daemon ran.
#[tokio::test]
async fn a_daemon_that_drops_the_hello_blocks_the_app_without_relaunching() {
    let daemon = OtherDaemon::start("silent");
    let launches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let launcher = Launcher::fake(daemon.platform.clone(), {
        let launches = launches.clone();
        move || {
            launches.fetch_add(1, Ordering::AcqRel);
            crate::launcher::Launched::exited(0)
        }
    });
    let bridge = bridge_for(&daemon, launcher, quick_waits());
    let reason = tokio::select! {
        reason = blocked_reason(&bridge) => reason,
        () = daemon.drop_hellos() => unreachable!(),
    };
    assert!(reason.contains("another version"), "{reason}");
    assert!(reason.contains("brigadierd"), "{reason}");
    // Longer than the launcher's cooldown, with the app checking every 200 ms meanwhile.
    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(3_500)) => {}
        () = daemon.drop_hellos() => unreachable!(),
    }
    assert_eq!(launches.load(Ordering::Acquire), 1, "launched again");
    stop(bridge);
    daemon.cleanup();
}

/// An older daemon that agrees to quit but doesn't go, and one that never answers: each is
/// waited for no longer than its deadline, and the app says which, and how to clear it.
#[tokio::test]
async fn an_older_daemon_that_doesnt_quit_is_waited_for_within_a_deadline() {
    for answers in [true, false] {
        let daemon = OtherDaemon::start(if answers { "stays" } else { "mute" });
        let launcher = Launcher::fake(daemon.platform.clone(), crate::launcher::Launched::running);
        let started = tokio::time::Instant::now();
        let bridge = bridge_for(&daemon, launcher, quick_waits());
        let mut old = daemon.next().await;
        // Still running: this test's own process.
        old.writer
            .write(&ServerFrame::ProtocolMismatch {
                daemon: daemon_info(PROTOCOL_VERSION - 1, std::process::id()),
            })
            .await
            .unwrap();
        let (id, request) = next_request(&mut old).await.unwrap();
        assert!(matches!(request, Request::Shutdown));
        if answers {
            old.writer
                .write(&ServerFrame::Response {
                    id,
                    result: Outcome::Ok {
                        value: Box::new(Response::Shutdown),
                    },
                })
                .await
                .unwrap();
        }
        let reason = blocked_reason(&bridge).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        if answers {
            assert!(reason.contains("still running"), "{reason}");
        } else {
            assert!(reason.contains("didn't agree to quit"), "{reason}");
        }
        assert!(reason.contains(&std::process::id().to_string()), "{reason}");
        stop(bridge);
        drop(old);
        daemon.cleanup();
    }
}

/// A newer daemon (this app was downgraded) is left running, and the app says to update.
#[tokio::test]
async fn a_newer_daemon_is_left_running_and_the_app_says_to_update() {
    let daemon = OtherDaemon::start("newer");
    let launcher = Launcher::fake(daemon.platform.clone(), crate::launcher::Launched::running);
    let bridge = bridge_for(&daemon, launcher, quick_waits());
    let mut newer = daemon.next().await;
    newer
        .writer
        .write(&ServerFrame::ProtocolMismatch {
            daemon: daemon_info(PROTOCOL_VERSION + 1, std::process::id()),
        })
        .await
        .unwrap();
    let reason = blocked_reason(&bridge).await;
    assert!(reason.contains("newer than this app"), "{reason}");
    assert!(
        next_request(&mut newer).await.is_none(),
        "a newer daemon was asked for something"
    );
    stop(bridge);
    daemon.cleanup();
}

/// An older daemon that agrees to quit and stays: the app keeps saying it is blocked, and how
/// to clear it, through every later attempt, never falling back to a plain "disconnected".
#[tokio::test]
async fn the_app_stays_blocked_while_an_older_daemon_keeps_running() {
    let daemon = OtherDaemon::start("lingers");
    let launcher = Launcher::fake(daemon.platform.clone(), crate::launcher::Launched::running);
    let bridge = bridge_for(&daemon, launcher, quick_waits());
    // Every hello is answered as an older daemon that agrees to quit and is still running:
    // this test's own process.
    let lingering = async {
        loop {
            let mut old = daemon.next().await;
            old.writer
                .write(&ServerFrame::ProtocolMismatch {
                    daemon: daemon_info(PROTOCOL_VERSION - 1, std::process::id()),
                })
                .await
                .unwrap();
            let Some((id, _)) = next_request(&mut old).await else {
                continue;
            };
            old.writer
                .write(&ServerFrame::Response {
                    id,
                    result: Outcome::Ok {
                        value: Box::new(Response::Shutdown),
                    },
                })
                .await
                .unwrap();
        }
    };
    let watched = async {
        let first = blocked_reason(&bridge).await;
        // Several attempts: each waits 400 ms for the exit, then 200 ms.
        let until = tokio::time::Instant::now() + Duration::from_millis(2_500);
        while tokio::time::Instant::now() < until {
            let status = bridge.inner.status.lock().unwrap().clone();
            assert!(
                matches!(&status, Some(BridgeEvent::Blocked { reason }) if *reason == first),
                "no longer blocked: {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        () = watched => {}
        () = lingering => unreachable!(),
    }
    stop(bridge);
    daemon.cleanup();
}

/// A quit asked while an older daemon is being asked to quit launches no daemon after it.
#[tokio::test]
async fn a_quit_while_an_older_daemon_goes_launches_none() {
    let daemon = OtherDaemon::start("quitting");
    let launches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let launcher = Launcher::fake(daemon.platform.clone(), {
        let launches = launches.clone();
        move || {
            launches.fetch_add(1, Ordering::AcqRel);
            crate::launcher::Launched::running()
        }
    });
    let bridge = bridge_for(&daemon, launcher, Waits::default());
    let mut old = daemon.next().await;
    old.writer
        .write(&ServerFrame::ProtocolMismatch {
            daemon: daemon_info(PROTOCOL_VERSION - 1, GONE_PID),
        })
        .await
        .unwrap();
    let (id, _) = next_request(&mut old).await.unwrap();
    // The user quits while the older daemon drains.
    let quitting = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.shutdown_daemon(Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    old.writer
        .write(&ServerFrame::Response {
            id,
            result: Outcome::Ok {
                value: Box::new(Response::Shutdown),
            },
        })
        .await
        .unwrap();
    drop(old);
    assert!(!quitting.await.unwrap(), "no daemon of its own to stop");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        launches.load(Ordering::Acquire),
        0,
        "launched while quitting"
    );
    daemon.cleanup();
}
