//! Windows Connect without a real install, account, network request or browser. The real
//! PowerShell installer runner and both provider adapters talk to local command-file fakes.
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
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("agent.ps1"), AGENT).unwrap();
        std::fs::write(
            dir.join("agent.cmd"),
            "@echo off\r\npowershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%BRIGADIER_CONNECT_TEST_HOME%\\agent.ps1\" %*\r\nexit /b %errorlevel%\r\n",
        ).unwrap();
        std::fs::write(dir.join("install.ps1"), INSTALLER).unwrap();
        Self(dir)
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
New-Item -ItemType Directory -Force $bin | Out-Null
Copy-Item (Join-Path $root 'agent.cmd') (Join-Path $bin ($env:BRIGADIER_CONNECT_TEST_PROVIDER + '.cmd'))
"#;

const AGENT: &str = r#"
$root = $env:BRIGADIER_CONNECT_TEST_HOME
$signed = Test-Path (Join-Path $root 'signed-in')
if ($args[0] -eq '--version') { Write-Output '2.1.302'; exit 0 }
if ($args[0] -eq 'auth' -and $args[1] -eq 'status') {
    @{ loggedIn = $signed; authMethod = 'oauth' } | ConvertTo-Json -Compress
    exit 0
}
if ($args[0] -eq 'login' -and $args[1] -eq 'status') {
    if ($signed) { [Console]::Error.WriteLine('Logged in using a browser'); exit 0 }
    exit 1
}
$expected = if ($env:BRIGADIER_CONNECT_TEST_PROVIDER -eq 'claude') { 'auth login' } else { 'login' }
if (($args -join ' ') -ne $expected) { throw 'wrong sign-in command' }
Add-Content (Join-Path $root 'pids') $PID
Set-Content (Join-Path $root 'login-started') 'yes'
if (-not $env:BROWSER) { throw 'the sign-in must hand its link back' }
& $env:BROWSER $env:BRIGADIER_CONNECT_TEST_LINK
$until = [DateTime]::UtcNow.AddSeconds(10)
while (-not (Test-Path (Join-Path $root 'opened'))) {
    if ([DateTime]::UtcNow -gt $until) { throw 'Brigadier did not open the sign-in link' }
    Start-Sleep -Milliseconds 20
}
$mode = Get-Content (Join-Path $root 'login-mode') -ErrorAction SilentlyContinue
switch ($mode) {
    quit { exit 1 }
    wait { Start-Sleep 60; exit 1 }
}
Set-Content (Join-Path $root 'signed-in') 'yes'
if ($env:BRIGADIER_CONNECT_TEST_PROVIDER -eq 'codex') {
    [Console]::Error.WriteLine('Successfully logged in')
} else { Write-Output 'Login successful.' }
if ($mode -eq 'stay') { Start-Sleep 60 }
"#;

fn ops(home: &Home, provider: ProviderKind) -> Ops {
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
        install_timeout: Duration::from_secs(20),
        sign_in_timeout: Duration::from_secs(20),
        poll: Duration::from_millis(500),
        mark: Arc::new(|_| None),
        unwatched_poll: Duration::from_millis(500),
        linger: Duration::from_millis(100),
        open_link: Arc::new(move |link| std::fs::write(opened.join("opened"), link).unwrap()),
    }
}

fn no_codes() -> Codes {
    Codes {
        offer: Box::new(|_| {}),
        pasted: tokio::sync::mpsc::unbounded_channel().1,
    }
}

async fn run(ops: &Ops, provider: ProviderKind) -> (Option<ConnectProgress>, Vec<ConnectProgress>) {
    let seen = Mutex::new(Vec::new());
    let outcome = tokio::time::timeout(
        Duration::from_secs(45),
        connect(
            ops,
            provider,
            &CancellationToken::new(),
            &|step| {
                seen.lock().unwrap().push(step);
            },
            &mut no_codes(),
        ),
    )
    .await
    .expect("Connect ended")
    .map(|ended| ended.progress);
    (outcome, seen.into_inner().unwrap())
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
        let spec = installer_spec(&ops, provider);
        assert_eq!(spec.program, Path::new("powershell.exe"));
        assert_eq!(spec.cwd.as_ref(), Some(&home.0));
        assert!(spec.args.iter().any(|arg| arg == "-NonInteractive"));
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
                home.0
                    .join(".local/bin")
                    .join(format!("{}.cmd", provider.binary()))
            )
        );
        assert!(status.logged_in);
        assert_eq!(home.text("opened"), LINK);
        assert_eq!(home.text("tries").lines().count(), 1);
        assert_eq!(home.text("stdin-closed").trim(), "yes");
        stopped(&home, &ops).await;
        std::fs::remove_file(home.0.join("login-started")).unwrap();
        assert!(run(&ops, provider).await.1.is_empty());
        assert!(!home.0.join("login-started").exists());
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
    for mode in ["broken", "nothing"] {
        let home = Home::new();
        home.mode("install", mode);
        let ops = ops(&home, ProviderKind::Claude);
        assert!(matches!(
            run(&ops, ProviderKind::Claude).await.0,
            Some(ConnectProgress::Failed {
                step: ConnectStep::Install,
                ..
            })
        ));
        assert!(!home.0.join("login-started").exists());
        stopped(&home, &ops).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_installer_that_hangs_times_out() {
    let home = Home::new();
    home.mode("install", "hang");
    let mut ops = ops(&home, ProviderKind::Codex);
    ops.install_timeout = Duration::from_secs(5);
    assert!(matches!(
        run(&ops, ProviderKind::Codex).await.0,
        Some(ConnectProgress::Failed {
            step: ConnectStep::Install,
            ..
        })
    ));
    stopped(&home, &ops).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_failure_retry_and_connected_before_exit() {
    for provider in ProviderKind::ALL {
        let home = Home::new();
        home.mode("login", "quit");
        let ops = ops(&home, provider);
        assert!(matches!(
            run(&ops, provider).await.0,
            Some(ConnectProgress::Failed {
                step: ConnectStep::SignIn,
                ..
            })
        ));
        home.mode("login", "stay");
        let (outcome, seen) = run(&ops, provider).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert_eq!(seen, [ConnectProgress::SigningIn]);
        assert_eq!(home.text("tries").lines().count(), 1);
        stopped(&home, &ops).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_that_hangs_times_out_or_can_be_cancelled() {
    let home = Home::new();
    home.mode("login", "wait");
    let provider = ProviderKind::Codex;
    let mut ops = ops(&home, provider);
    ops.sign_in_timeout = Duration::from_secs(5);
    assert!(matches!(
        run(&ops, provider).await.0,
        Some(ConnectProgress::Failed {
            step: ConnectStep::SignIn,
            ..
        })
    ));
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
