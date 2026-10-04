//! Starts the same owned app bundle hidden when a run report needs a notification and no
//! app is connected. The app validates the exact outbox ID over authenticated IPC. It never
//! uses Script Editor or the installed app's data as a substitute for the right identity.

use crate::server::Daemon;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub async fn deliver(
    daemon: Arc<Daemon>,
    data_dir: PathBuf,
    stop: CancellationToken,
) -> anyhow::Result<()> {
    let mut attempts = HashMap::<String, Instant>::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! { () = stop.cancelled() => return Ok(()), _ = tick.tick() => {} }
        let notices = daemon.sessions.pending_overnight_notifications().await;
        attempts.retain(|id, _| notices.iter().any(|(_, _, notice)| &notice.id == id));
        if notices.is_empty() || daemon.metrics.clients() > 0 {
            continue;
        }
        let Ok(exe) = tokio::fs::read_to_string(data_dir.join("overnight-notification-host")).await
        else {
            continue;
        };
        let exe = PathBuf::from(exe);
        if !exe.is_file() {
            continue;
        }
        for (_, _, notice) in notices {
            // Starting the host again changes nothing while notifications are off.
            if notice.delivery_error.as_deref()
                == Some(brigadier_core::overnight::NOTIFICATIONS_OFF)
            {
                continue;
            }
            if attempts
                .get(&notice.id)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
            {
                continue;
            }
            attempts.insert(notice.id.clone(), Instant::now());
            let mut command;
            #[cfg(target_os = "macos")]
            {
                // LaunchServices starts it in the background and forwards a concurrent launch
                // to its existing instance. A bare target binary has no usable native identity.
                let Some(bundle) = exe
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                else {
                    continue;
                };
                if bundle.extension().is_none_or(|ext| ext != "app") {
                    continue;
                }
                command = tokio::process::Command::new("/usr/bin/open");
                command.arg("-g").arg("-a").arg(bundle).arg("--args");
            }
            #[cfg(not(target_os = "macos"))]
            {
                command = tokio::process::Command::new(&exe);
            }
            command
                .arg("--overnight-notification")
                .arg(&notice.id)
                .arg("--brigadier-data-dir")
                .arg(&data_dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(cfg!(target_os = "macos"));
            #[cfg(target_os = "macos")]
            match tokio::time::timeout(Duration::from_secs(10), command.status()).await {
                Ok(Ok(status)) if status.success() => {
                    tracing::info!(notification = %notice.id, "started Brigadier's hidden notification host")
                }
                result => {
                    tracing::warn!(result = ?result, "could not launch the notification host; report retained")
                }
            }
            #[cfg(not(target_os = "macos"))]
            match command.spawn() {
                Ok(mut child) => {
                    tokio::spawn(async move {
                        let _ = child.wait().await;
                    });
                }
                Err(err) => {
                    tracing::warn!(error = %err, "could not launch the notification host; report retained")
                }
            }
        }
    }
}
