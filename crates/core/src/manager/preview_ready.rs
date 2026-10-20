//! When a starting preview is ready to look at ([`super::preview`]): a web preview once the
//! local URL it printed, or a TCP port a process of its process group listens on, answers an
//! HTTP request; any preview once a process of its group owns an on-screen window. Members are
//! found by process group, so one a launcher left behind (reparented) still counts. Every probe
//! is cheap and bounded, and one that can't run (no `lsof`, no window server) reads as "not
//! ready", never as an error.

use std::time::Duration;

use crate::work::PreviewKind;

/// How long one connection or HTTP answer may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long `lsof` may take.
#[cfg(target_os = "macos")]
const LSOF_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(target_os = "macos")]
const LSOF: &str = "/usr/sbin/lsof";

/// The kind of preview `command` starts: an app for the dev commands of Tauri and Electron,
/// which open their own window (whatever frontend address they print is not the app), and for a
/// macOS app bundle's binary run directly (a dev app built and launched in one command); a web
/// preview for anything else.
pub(super) fn infer_kind(command: &str) -> PreviewKind {
    let lower = command.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
        .map(|word| word.trim_matches(['\'', '"']))
        .filter(|word| !word.is_empty() && *word != "--")
        .collect();
    let program = |word: &str, name: &str| word == name || word.ends_with(&format!("/{name}"));
    let app = words.iter().enumerate().any(|(at, word)| {
        let next = words.get(at + 1).copied().unwrap_or_default();
        word.contains("tauri:dev")
            || (program(word, "tauri") && next == "dev")
            || program(word, "electron")
            || (program(word, "electron-forge") && next == "start")
            || (program(word, "electron-vite") && next == "dev")
            || word.contains(".app/contents/macos/")
    });
    let after = |at: usize, wanted: &dyn Fn(&str) -> bool| {
        words.iter().skip(at + 1).take(3).any(|word| wanted(word))
    };
    let runners = [
        "cargo",
        "npm",
        "pnpm",
        "yarn",
        "bun",
        "go",
        "swift",
        "dotnet",
        "deno",
        "mix",
        "xcodebuild",
        "gradle",
        "gradlew",
        "mvn",
        "make",
        "flutter",
    ];
    let test = words.iter().enumerate().any(|(at, word)| {
        [
            "pytest", "jest", "vitest", "rspec", "phpunit", "mocha", "ctest",
        ]
        .iter()
        .any(|name| program(word, name))
            || (runners.iter().any(|name| program(word, name))
                && after(at, &|next| {
                    next == "test" || next == "nextest" || next.starts_with("test:")
                }))
    });
    let simulator = words.iter().enumerate().any(|(at, word)| {
        *word == "simctl"
            || (program(word, "xcodebuild") && words.iter().any(|w| w.contains("simulator")))
            || (*word == "simulator" && at > 0 && words[at - 1] == "-a")
    });
    if app {
        PreviewKind::App
    } else if test {
        PreviewKind::Test
    } else if simulator {
        PreviewKind::Simulator
    } else if words.iter().any(|word| word.ends_with(".pdf")) {
        PreviewKind::Pdf
    } else {
        PreviewKind::Web
    }
}

/// The command's words as a shell splits them, quotes taken off and case kept.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')') => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// What a preview of `kind` started by `command` shows when that isn't a URL: a simulator's
/// device (a destination's `name=`, `simctl`'s device, else "iOS Simulator"), a PDF's file
/// (the command's last `.pdf`), a test run's command.
pub(super) fn infer_target(kind: PreviewKind, command: &str) -> Option<String> {
    let words = shell_words(command);
    match kind {
        PreviewKind::Web | PreviewKind::App => None,
        PreviewKind::Test => Some(command.trim().to_owned()),
        PreviewKind::Pdf => words
            .iter()
            .rfind(|word| word.to_lowercase().ends_with(".pdf"))
            .cloned(),
        PreviewKind::Simulator => {
            let named = words.iter().find_map(|word| {
                let at = word.find("name=")?;
                let name = word[at + "name=".len()..].split(',').next()?.trim();
                (!name.is_empty()).then(|| name.to_owned())
            });
            let device = || {
                let at = words.iter().position(|word| word == "simctl")?;
                let verb = words.get(at + 1)?;
                ["boot", "launch", "install", "openurl", "io"]
                    .contains(&verb.as_str())
                    .then(|| words.get(at + 2))
                    .flatten()
                    .filter(|device| !device.starts_with('-') && device.as_str() != "booted")
                    .cloned()
            };
            Some(
                named
                    .or_else(device)
                    .unwrap_or_else(|| "iOS Simulator".to_owned()),
            )
        }
    }
}

/// Whether [`window_owners`] can see windows here. Where it can't, an app preview has no
/// readiness signal and reads as running once started, as every preview did before.
pub(super) const SEES_WINDOWS: bool = cfg!(target_os = "macos");

/// The processes that own an on-screen window. The owner of a window is readable without the
/// Screen Recording permission.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(super) fn window_owners() -> Vec<u32> {
    use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use objc2_core_graphics::{
        CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowLayer, kCGWindowOwnerPID,
    };

    let Some(list) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    ) else {
        return Vec::new();
    };
    // SAFETY: the window list is an array of dictionaries with string keys.
    let list: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(list) };
    // SAFETY: reading the system's constant keys.
    let (k_layer, k_pid) = unsafe { (kCGWindowLayer, kCGWindowOwnerPID) };
    let num = |d: &CFDictionary<CFString, CFType>, k: &CFString| -> Option<i64> {
        d.get(k)
            .and_then(|v| v.downcast::<CFNumber>().ok())
            .and_then(|n| n.as_i64())
    };
    let mut owners: Vec<u32> = list
        .iter()
        .filter(|window| num(window, k_layer) == Some(0))
        .filter_map(|window| u32::try_from(num(&window, k_pid)?).ok())
        .collect();
    owners.sort_unstable();
    owners.dedup();
    owners
}

#[cfg(not(target_os = "macos"))]
pub(super) fn window_owners() -> Vec<u32> {
    Vec::new()
}

/// The TCP ports the processes of process group `group` listen on, per `lsof`; none when it
/// is missing or fails.
#[cfg(target_os = "macos")]
pub(super) async fn listening_ports(group: u32) -> Vec<u16> {
    lsof_listening(LSOF, "-g", group).await.unwrap_or_default()
}

/// The TCP ports process `pid` listens on, per `lsof`; `None` when that can't be told (`lsof`
/// missing, failing or too slow), which a caller sparing listening servers must read as
/// "may listen".
#[cfg(target_os = "macos")]
pub(super) async fn process_listening_ports(pid: u32) -> Option<Vec<u16>> {
    lsof_listening(LSOF, "-p", pid).await
}

/// The TCP ports the processes `lsof` selects with `flag` `id` (`-g` a group, `-p` a process)
/// listen on; `None` when `lsof` can't run or doesn't finish in time.
#[cfg(target_os = "macos")]
async fn lsof_listening(lsof: &str, flag: &str, id: u32) -> Option<Vec<u16>> {
    let id = id.to_string();
    let output = tokio::process::Command::new(lsof)
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-Fn", flag, &id])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(LSOF_TIMEOUT, output).await {
        // It exits 1 when nothing matches, so only its running at all tells.
        Ok(Ok(output)) => Some(lsof_ports(&String::from_utf8_lossy(&output.stdout))),
        _ => None,
    }
}

/// The ports of `lsof -Fn` lines such as `n*:3000`, `n127.0.0.1:5173` and `n[::1]:8080`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn lsof_ports(output: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = output
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .filter_map(|name| name.rsplit_once(':')?.1.parse().ok())
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// The TCP ports the processes of process group `group` listen on: the listening sockets of
/// `/proc/net/tcp{,6}` that one of their open files is.
#[cfg(target_os = "linux")]
pub(super) async fn listening_ports(group: u32) -> Vec<u16> {
    // The process group is the fifth field of `stat`, the third after the command's `)`.
    proc_listening(move |pid| {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(2))
                .and_then(|pgrp| pgrp.parse::<u32>().ok())
                == Some(group)
        })
    })
    .await
    .unwrap_or_default()
}

/// The TCP ports process `pid` listens on; `None` when that can't be told (no socket table
/// readable), which a caller sparing listening servers must read as "may listen".
#[cfg(target_os = "linux")]
pub(super) async fn process_listening_ports(pid: u32) -> Option<Vec<u16>> {
    let pid = pid.to_string();
    proc_listening(move |of| of == pid).await
}

/// The TCP ports the processes `wanted` picks (by pid) listen on: the listening sockets of
/// `/proc/net/tcp{,6}` that one of their open files is. `None` when neither table can be read.
#[cfg(target_os = "linux")]
async fn proc_listening(wanted: impl Fn(&str) -> bool + Send + 'static) -> Option<Vec<u16>> {
    tokio::task::spawn_blocking(move || {
        let mut listening = std::collections::HashMap::new();
        let mut read_any = false;
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(table) else {
                continue;
            };
            read_any = true;
            for line in text.lines().skip(1) {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.get(3) != Some(&"0A") {
                    continue;
                }
                let port = fields
                    .get(1)
                    .and_then(|local| local.rsplit_once(':'))
                    .and_then(|(_, port)| u16::from_str_radix(port, 16).ok());
                if let (Some(port), Some(inode)) = (port, fields.get(9)) {
                    listening.insert(format!("socket:[{inode}]"), port);
                }
            }
        }
        let mut ports = Vec::new();
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return None;
        };
        if !read_any {
            return None;
        }
        for process in processes.flatten() {
            let pid = process.file_name().to_string_lossy().into_owned();
            if !pid.bytes().all(|b| b.is_ascii_digit()) || !wanted(&pid) {
                continue;
            }
            let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                if let Ok(target) = std::fs::read_link(fd.path())
                    && let Some(port) = listening.get(&*target.to_string_lossy())
                {
                    ports.push(*port);
                }
            }
        }
        ports.sort_unstable();
        ports.dedup();
        Some(ports)
    })
    .await
    .ok()
    .flatten()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) async fn listening_ports(_group: u32) -> Vec<u16> {
    Vec::new()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) async fn process_listening_ports(_pid: u32) -> Option<Vec<u16>> {
    None
}

/// Whether an HTTP server on `port` of this machine answers (a TCP connect alone isn't enough:
/// a dev server may accept before it can serve).
pub(super) async fn port_answers(port: u16) -> bool {
    url_answers(&format!("http://127.0.0.1:{port}/")).await
        || url_answers(&format!("http://[::1]:{port}/")).await
}

/// Whether a local `url` (as [`super::preview`] finds them: http or https, on localhost or
/// 127.0.0.1) answers: any HTTP response to a GET for http. Brigadier has no TLS client, so for
/// https an accepted connection counts.
pub(super) async fn url_answers(url: &str) -> bool {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let (https, rest) = match (url.strip_prefix("http://"), url.strip_prefix("https://")) {
        (Some(rest), _) => (false, rest),
        (_, Some(rest)) => (true, rest),
        _ => return false,
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = (&rest[..end], &rest[end..]);
    let path = if path.starts_with('/') { path } else { "/" };
    let address = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:{}", if https { 443 } else { 80 })
    };
    let probe = async {
        let mut stream = tokio::net::TcpStream::connect(address).await.ok()?;
        if https {
            return Some(true);
        }
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut start = [0; 5];
        stream.read_exact(&mut start).await.ok()?;
        Some(&start == b"HTTP/")
    };
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, probe).await,
        Ok(Some(true))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tauri_and_electron_dev_commands_are_apps_and_the_rest_web() {
        for app in [
            "pnpm tauri dev",
            "cargo tauri dev --config '{\"identifier\":\"ai.brigadier.x\"}'",
            "npm run tauri -- dev",
            "pnpm tauri:dev",
            "BRIGADIER_DATA_DIR=/tmp/x pnpm run tauri:dev",
            "electron .",
            "./node_modules/.bin/electron main.js --no-sandbox",
            "npx electron-forge start",
            "pnpm exec electron-vite dev",
            // A dev app built and launched in one command.
            "cd apps/desktop && pnpm tauri:debug-app && \"/w/target/debug/bundle/macos/Brigadier Dev.app/Contents/MacOS/brigadier-desktop\"",
        ] {
            assert_eq!(infer_kind(app), PreviewKind::App, "{app}");
        }
        for web in [
            "pnpm dev",
            "npm run dev -- --port 3000",
            "vite",
            "pnpm tauri build",
            "pnpm tauri:debug-app",
            "python3 -m http.server",
            "electron-builder --dir",
            "electron-vite build",
        ] {
            assert_eq!(infer_kind(web), PreviewKind::Web, "{web}");
        }
    }

    /// Test commands are test runs, simulator commands simulators and a `.pdf` a PDF, each
    /// with what it shows; an app's dev command stays an app.
    #[test]
    fn test_runs_simulators_and_pdfs_are_read_from_their_commands() {
        for (command, kind, target) in [
            (
                "cargo test -p core",
                PreviewKind::Test,
                Some("cargo test -p core"),
            ),
            (
                "pnpm --filter app test",
                PreviewKind::Test,
                Some("pnpm --filter app test"),
            ),
            (
                "npm run test:unit",
                PreviewKind::Test,
                Some("npm run test:unit"),
            ),
            (
                "python3 -m pytest -q",
                PreviewKind::Test,
                Some("python3 -m pytest -q"),
            ),
            ("go test ./...", PreviewKind::Test, Some("go test ./...")),
            (
                "xcodebuild -scheme Todo -destination 'platform=iOS Simulator,name=iPhone 16' build && xcrun simctl launch booted ai.todo",
                PreviewKind::Simulator,
                Some("iPhone 16"),
            ),
            (
                "xcrun simctl boot \"iPad Air\"",
                PreviewKind::Simulator,
                Some("iPad Air"),
            ),
            (
                "open -a Simulator",
                PreviewKind::Simulator,
                Some("iOS Simulator"),
            ),
            (
                "pandoc README.md -o out/Report.pdf && open out/Report.pdf",
                PreviewKind::Pdf,
                Some("out/Report.pdf"),
            ),
            ("pnpm dev", PreviewKind::Web, None),
            ("pnpm tauri dev", PreviewKind::App, None),
            ("pnpm testing-server", PreviewKind::Web, None),
        ] {
            let found = infer_kind(command);
            assert_eq!(found, kind, "{command}");
            assert_eq!(infer_target(found, command).as_deref(), target, "{command}");
        }
    }

    #[test]
    fn lsof_names_give_their_ports() {
        assert_eq!(
            lsof_ports("p12\nf5\nn*:3000\nf6\nn127.0.0.1:5173\np13\nf7\nn[::1]:5173\n"),
            vec![3000, 5173]
        );
        assert_eq!(lsof_ports(""), Vec::<u16>::new());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_missing_lsof_reads_as_unknown_not_as_not_listening() {
        let own = std::process::id();
        assert_eq!(lsof_listening("/nonexistent/lsof", "-p", own).await, None);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ports = process_listening_ports(own).await.expect("lsof ran");
        assert!(ports.contains(&port), "{ports:?} lacks {port}");
    }

    #[tokio::test]
    async fn a_url_answers_once_a_server_responds_and_a_port_once_it_accepts() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/app");
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 1024];
            let read = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..read]).into_owned()
        });
        assert!(url_answers(&url).await);
        assert!(server.await.unwrap().starts_with("GET /app HTTP/1.1\r\n"));
        assert!(!port_answers(port).await);
        // Nothing listens there any more.
        assert!(!url_answers(&url).await);
        assert!(!url_answers("ftp://localhost/").await);
    }

    #[tokio::test]
    async fn a_server_that_accepts_but_does_not_answer_yet_is_not_ready() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (answer_tx, mut answer_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                if *answer_rx.borrow_and_update() {
                    // The request read first, as a server does: Windows resets a connection
                    // closed with input unread, and the answer with it.
                    let mut request = [0; 1024];
                    let _ = stream.read(&mut request).await;
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
                } else {
                    // Accepted, never answered: held until the probe gives up.
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        drop(stream);
                    });
                }
            }
        });
        assert!(!port_answers(port).await);
        answer_tx.send(true).unwrap();
        assert!(port_answers(port).await);
    }
}
