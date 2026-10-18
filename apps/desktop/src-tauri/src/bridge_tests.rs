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
            Box::new(move |title, body| {
                shown.send((title.to_owned(), body.to_owned())).unwrap();
            }),
            Box::new(|| false),
            Box::new(|| {}),
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
