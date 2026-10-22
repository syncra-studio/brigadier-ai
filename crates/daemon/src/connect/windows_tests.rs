//! Windows Connect without a real install, account, network request or browser. The real
//! PowerShell installer runner and both provider adapters talk to local native fakes.
use std::path::Path;

use brigadier_providers::Provider;
use brigadier_providers::claude::Claude;
use brigadier_providers::codex::Codex;

use super::*;

const LINK: &str = "https://example.invalid/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A5512%2Fcallback&state=a!b%25c";

struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "20261020-1537-issues-sweep-w10-connect space-{}",
            uuid::Uuid::new_v4()
        ));
        let home = Self(dir);
        let dir = &home.0;
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let source = dir.join("fake-agent.rs");
        std::fs::write(&source, include_str!("fake_agent.rs")).unwrap();
        let output = std::process::Command::new("rustc")
            .arg("--edition=2024")
            .arg("--crate-name=brigadier_connect_fake")
            .arg(&source)
            .arg("-o")
            .arg(dir.join("fake-agent.exe"))
            .output()
            .expect("the Rust toolchain that runs these tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::write(dir.join("install.ps1"), INSTALLER).unwrap();
        home
    }

    /// Where the vendor's installer puts the agent: Codex's in `%LOCALAPPDATA%`.
    fn bin(&self, provider: ProviderKind) -> PathBuf {
        match provider {
            ProviderKind::Claude => self.0.join(r".local\bin"),
            ProviderKind::Codex => self.0.join(r"AppData\Local\Programs\OpenAI\Codex\bin"),
        }
    }

    fn install(&self, provider: ProviderKind) {
        let bin = self.bin(provider);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(
            self.0.join("fake-agent.exe"),
            bin.join(format!("{}.exe", provider.binary())),
        )
        .unwrap();
    }

    fn mode(&self, stage: &str, mode: &str) {
        std::fs::write(self.0.join(format!("{stage}-mode")), mode).unwrap();
    }

    fn text(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).unwrap_or_default()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const INSTALLER: &str = r#"
$root = $env:BRIGADIER_CONNECT_TEST_HOME
Add-Content (Join-Path $root 'tries') 'try'
Add-Content (Join-Path $root 'pids') $PID
if ($env:CODEX_NON_INTERACTIVE -ne '1') { throw 'installer must not ask questions' }
if ($null -ne [Console]::In.ReadLine()) { throw 'installer stdin must be closed' }
Set-Content (Join-Path $root 'stdin-closed') 'yes'
$mode = Get-Content (Join-Path $root 'install-mode') -ErrorAction SilentlyContinue
switch ($mode) {
    offline { Write-Error 'curl: (7) Failed to connect to the download server'; exit 0 }
    broken { exit 137 }
    nothing { exit 0 }
    hang { Start-Sleep 60; exit 0 }
}
$bin = Join-Path $root '.local\bin'
if ($env:BRIGADIER_CONNECT_TEST_PROVIDER -eq 'codex') { $bin = Join-Path $env:LOCALAPPDATA 'Programs\OpenAI\Codex\bin' }
New-Item -ItemType Directory -Force $bin | Out-Null
Copy-Item (Join-Path $root 'fake-agent.exe') (Join-Path $bin ($env:BRIGADIER_CONNECT_TEST_PROVIDER + '.exe'))
"#;

fn ops(home: &Home, provider: ProviderKind) -> Ops {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(home.0.join("data")),
    })
    .unwrap();
    let system = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system folder"));
    let path = std::env::join_paths([
        home.0.join("bin"),
        system.join("System32/WindowsPowerShell/v1.0"),
        system.join("System32"),
        system.clone(),
    ])
    .unwrap();
    let mut vars = std::collections::BTreeMap::from([
        ("HOME".into(), home.0.clone().into_os_string()),
        ("USERPROFILE".into(), home.0.clone().into_os_string()),
        (
            "LOCALAPPDATA".into(),
            home.0.join(r"AppData\Local").into_os_string(),
        ),
        (
            "BRIGADIER_CONNECT_TEST_HOME".into(),
            home.0.clone().into_os_string(),
        ),
        (
            "BRIGADIER_CONNECT_TEST_PROVIDER".into(),
            provider.binary().into(),
        ),
        ("BRIGADIER_CONNECT_TEST_LINK".into(), LINK.into()),
        ("PATH".into(), path),
        ("SystemRoot".into(), system.into_os_string()),
        ("PATHEXT".into(), ".COM;.EXE;.BAT;.CMD".into()),
    ]);
    for key in ["TEMP", "TMP", "COMSPEC"] {
        if let Some(value) = std::env::var_os(key) {
            vars.insert(key.into(), value);
        }
    }
    let env = Arc::new(CliEnv::from_vars(vars));
    let agent: Arc<dyn Provider> = match provider {
        ProviderKind::Claude => Arc::new(Claude::new(platform.clone(), env.clone())),
        ProviderKind::Codex => Arc::new(Codex::new(platform.clone(), env.clone())),
    };
    let opened = home.0.clone();
    Ops {
        platform,
        env,
        status: Arc::new(move |_| {
            let agent = agent.clone();
            Box::pin(async move { agent.status().await })
        }),
        installer: Arc::new(|_| {
            "& (Join-Path $env:BRIGADIER_CONNECT_TEST_HOME 'install.ps1')".into()
        }),
        install_timeout: Duration::from_secs(60),
        sign_in_timeout: Duration::from_secs(20),
        poll: Duration::from_millis(500),
        mark: Arc::new(|_| None),
        unwatched_poll: Duration::from_millis(500),
        linger: Duration::from_millis(100),
        open_link: Arc::new(move |link| std::fs::write(opened.join("opened"), link).unwrap()),
        add_to_path: path_written_to(home.0.join("path-added")),
    }
}

/// Never the real user's Path: each folder Connect adds is written down in `file` (user_path.rs's
/// own tests use a scratch registry key).
fn path_written_to(file: PathBuf) -> AddToPath {
    Arc::new(move |folder| {
        let mut added = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)?;
        std::io::Write::write_all(&mut added, format!("{}\n", folder.display()).as_bytes())?;
        Ok(user_path::Added::Appended)
    })
}

fn no_codes() -> Codes {
    Codes {
        offer: Box::new(|_| {}),
        pasted: tokio::sync::mpsc::unbounded_channel().1,
    }
}

async fn run(ops: &Ops, provider: ProviderKind) -> (Option<ConnectProgress>, Vec<ConnectProgress>) {
    let seen = Mutex::new(Vec::new());
    let cancel = CancellationToken::new();
    let report = |step| seen.lock().unwrap().push(step);
    let mut codes = no_codes();
    let attempt = connect(ops, provider, &cancel, &report, &mut codes);
    tokio::pin!(attempt);
    let outcome = match tokio::time::timeout(Duration::from_secs(180), &mut attempt).await {
        Ok(outcome) => outcome.map(|ended| ended.progress),
        Err(_) => {
            // Give Connect its cancellation path before a failed test drops the runtime.
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(10), &mut attempt).await;
            panic!("Connect did not end; steps: {:?}", seen.lock().unwrap());
        }
    };
    eprintln!("{provider}: {outcome:?}; steps: {:?}", seen.lock().unwrap());
    let steps = seen.lock().unwrap().clone();
    (outcome, steps)
}

async fn stopped(home: &Home, ops: &Ops) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let alive = home.text("pids").lines().any(|pid| {
            pid.trim()
                .parse::<u32>()
                .is_ok_and(|pid| ops.platform.processes().is_alive(pid))
        });
        if !alive {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a fake process survived Connect"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn both_agents_install_then_sign_in_and_hand_the_link_back() {
    for provider in ProviderKind::ALL {
        let home = Home::new();
        let ops = ops(&home, provider);
        assert!((ops.status)(provider).await.path.is_none());
        let spec = installer_spec(&ops, provider).unwrap();
        assert_eq!(spec.program, Path::new("powershell.exe"));
        assert_eq!(spec.cwd.as_ref(), Some(&home.0));
        assert!(spec.args.iter().any(|arg| arg == "-NonInteractive"));
        // The system's tar and curl, not Git's, for the vendor's install.ps1.
        let path = spec.env.iter().find(|(key, _)| key == "PATH").unwrap();
        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        assert_eq!(std::env::split_paths(&path.1).next(), Some(system));
        let (outcome, seen) = run(&ops, provider).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert_eq!(
            seen,
            [ConnectProgress::Installing, ConnectProgress::SigningIn]
        );
        let status = (ops.status)(provider).await;
        assert_eq!(
            status.path.map(PathBuf::from),
            Some(
                home.bin(provider)
                    .join(format!("{}.exe", provider.binary()))
            )
        );
        assert!(status.logged_in);
        assert_eq!(home.text("opened"), LINK);
        assert_eq!(home.text("tries").lines().count(), 1);
        assert_eq!(home.text("stdin-closed").trim(), "yes");
        // Claude Code's folder goes on the user's Path; Codex's installer adds its own.
        let added = match provider {
            ProviderKind::Claude => format!("{}\n", home.bin(provider).display()),
            ProviderKind::Codex => String::new(),
        };
        assert_eq!(home.text("path-added"), added);
        stopped(&home, &ops).await;
        std::fs::remove_file(home.0.join("login-started")).unwrap();
        assert!(run(&ops, provider).await.1.is_empty());
        assert!(!home.0.join("login-started").exists());
        // Only an install adds it.
        assert_eq!(home.text("path-added"), added);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_downloads_fail_instead_of_piping_nothing_and_retry_works() {
    for provider in ProviderKind::ALL {
        let home = Home::new();
        home.mode("install", "offline");
        let ops = ops(&home, provider);
        let (outcome, seen) = run(&ops, provider).await;
        assert_eq!(
            outcome,
            Some(ConnectProgress::Failed {
                step: ConnectStep::Install,
                error: format!(
                    "Brigadier couldn't download {}. Check your internet connection, then try again.",
                    provider.label()
                ),
                reason: None,
            })
        );
        assert_eq!(seen, [ConnectProgress::Installing]);
        assert!(!home.0.join("login-started").exists());
        home.mode("install", "finish");
        assert_eq!(
            run(&ops, provider).await.0,
            Some(ConnectProgress::Connected)
        );
        assert_eq!(home.text("tries").lines().count(), 2);
        stopped(&home, &ops).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_install_error_or_missing_binary_never_starts_sign_in() {
    for (mode, error) in [
        ("broken", "Claude Code couldn't be installed. Try again."),
        (
            "nothing",
            "Claude Code was installed, but Brigadier can't find it. Try again.",
        ),
    ] {
        let home = Home::new();
        home.mode("install", mode);
        let mut ops = ops(&home, ProviderKind::Claude);
        // The fake installer exits at once; only process start-up on a loaded CI runner can take
        // a while. A high timeout keeps that case resolving by exit code, not by racing the
        // timeout's wall-clock sleep, so the test can't flake into "took too long" instead.
        ops.install_timeout = Duration::from_secs(150);
        assert_eq!(
            run(&ops, ProviderKind::Claude).await.0,
            Some(ConnectProgress::Failed {
                step: ConnectStep::Install,
                error: error.into(),
                reason: None,
            })
        );
        assert!(!home.0.join("login-started").exists());
        // A failed install puts nothing on Path.
        assert_eq!(home.text("path-added"), "");
        stopped(&home, &ops).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_installer_that_hangs_times_out() {
    let home = Home::new();
    home.mode("install", "hang");
    let mut ops = ops(&home, ProviderKind::Codex);
    ops.install_timeout = Duration::from_secs(5);
    assert_eq!(
        run(&ops, ProviderKind::Codex).await.0,
        Some(ConnectProgress::Failed {
            step: ConnectStep::Install,
            error:
                "Installing Codex took too long. Check your internet connection, then try again."
                    .into(),
            reason: None,
        })
    );
    stopped(&home, &ops).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_failure_retry_and_connected_before_exit() {
    for provider in ProviderKind::ALL {
        let home = Home::new();
        home.install(provider);
        home.mode("login", "quit");
        let ops = ops(&home, provider);
        assert_eq!(
            run(&ops, provider).await.0,
            Some(ConnectProgress::Failed {
                step: ConnectStep::SignIn,
                error: format!(
                    "Signing in to {} didn't finish. Try again.",
                    provider.label()
                ),
                reason: None,
            })
        );
        home.mode("login", "stay");
        let (outcome, seen) = run(&ops, provider).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert_eq!(seen, [ConnectProgress::SigningIn]);
        assert_eq!(home.text("tries").lines().count(), 0);
        // Installed already, not by Connect: Path is left alone.
        assert_eq!(home.text("path-added"), "");
        stopped(&home, &ops).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_that_hangs_times_out_or_can_be_cancelled() {
    let home = Home::new();
    home.mode("login", "wait");
    let provider = ProviderKind::Codex;
    home.install(provider);
    let mut ops = ops(&home, provider);
    ops.sign_in_timeout = Duration::from_secs(5);
    assert_eq!(
        run(&ops, provider).await.0,
        Some(ConnectProgress::Failed {
            step: ConnectStep::SignIn,
            error: "Signing in to Codex took too long. Try again.".into(),
            reason: None,
        })
    );
    stopped(&home, &ops).await;
    std::fs::remove_file(home.0.join("login-started")).unwrap();
    ops.sign_in_timeout = Duration::from_secs(20);
    let cancel = CancellationToken::new();
    let mut codes = no_codes();
    let attempt = connect(&ops, provider, &cancel, &|_| {}, &mut codes);
    let stop = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while !home.0.join("login-started").exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "sign-in did not start"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        cancel.cancel();
    };
    let (outcome, ()) = tokio::join!(attempt, stop);
    assert!(outcome.is_none());
    stopped(&home, &ops).await;
}

#[test]
fn the_link_helper_rejects_non_web_or_extra_arguments() {
    for args in [
        vec!["file:///private"],
        vec!["https://example.invalid/\nother"],
        vec![LINK, "extra"],
    ] {
        let catcher = LinkCatcher::new().unwrap();
        let status = std::process::Command::new(catcher.script())
            .env("BRIGADIER_SIGN_IN_LINK_FILE", catcher.dir.join("link"))
            .args(args)
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(catcher.link().is_none());
    }
}

/// Connect with the vendor's real installer and sign-in, in a scratch home, up to the sign-in's
/// link: `BRIGADIER_CONNECT_LIVE` names the agent (`claude` or `codex`),
/// `BRIGADIER_CONNECT_LIVE_HOME` the home (left for the caller to look at and remove), and
/// `BRIGADIER_CONNECT_LIVE_PATH_FIRST` a folder put first on PATH (Git's tools, as Git Bash
/// has them). It downloads the agent, and its installer may add its folder to the user's PATH
/// in the registry: back that up first. No account is used: the sign-in is cancelled once its
/// link reaches Brigadier, or a minute after it starts (an agent that ignores `BROWSER` opens
/// the page itself). Codex finds its home through Windows' profile folder, not `USERPROFILE`
/// (codex-cli 0.163.0 read and wrote the user's own `.codex` here), so `CODEX_HOME` keeps it in
/// the scratch home.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "downloads a real agent and may change the user's PATH; run by hand"]
async fn live_install_up_to_the_sign_in_link() {
    let (Ok(agent), Some(home)) = (
        std::env::var("BRIGADIER_CONNECT_LIVE"),
        std::env::var_os("BRIGADIER_CONNECT_LIVE_HOME"),
    ) else {
        return;
    };
    let provider = match agent.as_str() {
        "claude" => ProviderKind::Claude,
        "codex" => ProviderKind::Codex,
        other => panic!("BRIGADIER_CONNECT_LIVE names no agent: {other}"),
    };
    let home = PathBuf::from(home);
    let system = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system folder"));
    let first = std::env::var_os("BRIGADIER_CONNECT_LIVE_PATH_FIRST").map(PathBuf::from);
    let path = std::env::join_paths(first.into_iter().chain([
        system.join(r"System32\WindowsPowerShell\v1.0"),
        system.join("System32"),
        system.clone(),
    ]))
    .unwrap();
    let mut vars = std::collections::BTreeMap::from([
        ("HOME".into(), home.clone().into_os_string()),
        ("USERPROFILE".into(), home.clone().into_os_string()),
        (
            "APPDATA".into(),
            home.join(r"AppData\Roaming").into_os_string(),
        ),
        (
            "LOCALAPPDATA".into(),
            home.join(r"AppData\Local").into_os_string(),
        ),
        ("TEMP".into(), home.join("Temp").into_os_string()),
        ("TMP".into(), home.join("Temp").into_os_string()),
        ("CODEX_HOME".into(), home.join(".codex").into_os_string()),
        ("PATH".into(), path),
        ("SystemRoot".into(), system.into_os_string()),
    ]);
    for key in [
        "COMSPEC",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "OS",
        "windir",
    ] {
        if let Some(value) = std::env::var_os(key) {
            vars.insert(key.into(), value);
        }
    }
    for dir in ["AppData/Roaming", "AppData/Local", "Temp", ".codex"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(home.join("brigadier-data")),
    })
    .unwrap();
    let env = Arc::new(CliEnv::from_vars(vars));
    let agent: Arc<dyn Provider> = match provider {
        ProviderKind::Claude => Arc::new(Claude::new(platform.clone(), env.clone())),
        ProviderKind::Codex => Arc::new(Codex::new(platform.clone(), env.clone())),
    };
    let link = home.join("brigadier-opened-link");
    let _ = std::fs::remove_file(&link);
    let opened = link.clone();
    let marks = env.clone();
    let ops = Ops {
        platform,
        env: env.clone(),
        status: Arc::new(move |_| {
            let agent = agent.clone();
            Box::pin(async move { agent.status().await })
        }),
        installer: Arc::new(|provider| provider.install_command(true).to_owned()),
        install_timeout: INSTALL_TIMEOUT,
        sign_in_timeout: Duration::from_secs(5 * 60),
        poll: POLL_EVERY,
        mark: Arc::new(move |provider| LoginMark::of(provider, &marks)),
        unwatched_poll: UNWATCHED_POLL,
        linger: LINGER,
        open_link: Arc::new(move |link| std::fs::write(&opened, link).unwrap()),
        add_to_path: path_written_to(home.join("brigadier-path-added")),
    };
    assert!(
        (ops.status)(provider).await.path.is_none(),
        "{provider} counts as installed before its installer ran"
    );

    let seen = Mutex::new(Vec::new());
    let cancel = CancellationToken::new();
    let mut codes = no_codes();
    let report = |step| seen.lock().unwrap().push(step);
    let attempt = connect(&ops, provider, &cancel, &report, &mut codes);
    tokio::pin!(attempt);
    let mut signing = None;
    let outcome = loop {
        tokio::select! {
            outcome = &mut attempt => break outcome.map(|ended| ended.progress),
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
        if seen.lock().unwrap().contains(&ConnectProgress::SigningIn) {
            let since = *signing.get_or_insert_with(tokio::time::Instant::now);
            if link.exists() || since.elapsed() > Duration::from_secs(60) {
                cancel.cancel();
            }
        }
    };
    let steps = seen.lock().unwrap().clone();
    let status = (ops.status)(provider).await;
    let handed_back = std::fs::read_to_string(&link).ok();
    eprintln!("{provider}: outcome {outcome:?}; steps {steps:?}");
    eprintln!("{provider}: status {status:?}");
    eprintln!("{provider}: sign-in link handed back through BROWSER: {handed_back:?}");
    assert_eq!(outcome, None, "Connect ended before it was cancelled");
    assert_eq!(
        steps,
        [ConnectProgress::Installing, ConnectProgress::SigningIn]
    );
    let folder = env.installer_folder(provider).unwrap();
    assert_eq!(
        status.path.map(PathBuf::from),
        Some(folder.join(format!("{}.exe", provider.binary())))
    );
    assert!(!status.logged_in);
}
