//! A native CLI stand-in compiled by the Windows Connect tests. Only its scratch files and
//! the test's BROWSER helper are used; it never opens a browser or contacts a provider.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let root = PathBuf::from(std::env::var_os("BRIGADIER_CONNECT_TEST_HOME").unwrap());
    let args: Vec<_> = std::env::args().skip(1).collect();
    let signed = root.join("signed-in").exists();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version"] => {
            println!("2.1.302");
            return ExitCode::SUCCESS;
        }
        ["auth", "status", "--json"] => {
            println!(r#"{{"loggedIn":{signed},"authMethod":"oauth"}}"#);
            return ExitCode::SUCCESS;
        }
        ["login", "status"] => {
            if signed {
                eprintln!("Logged in using a browser");
                return ExitCode::SUCCESS;
            }
            return ExitCode::FAILURE;
        }
        _ => {}
    }
    let provider = std::env::var("BRIGADIER_CONNECT_TEST_PROVIDER").unwrap();
    let expected = if provider == "claude" {
        "auth login"
    } else {
        "login"
    };
    assert_eq!(args.join(" "), expected, "wrong sign-in command");
    let mut pids = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("pids"))
        .unwrap();
    writeln!(pids, "{}", std::process::id()).unwrap();
    fs::write(root.join("login-started"), "yes").unwrap();
    let status = Command::new(std::env::var_os("BROWSER").expect("a link catcher"))
        .arg(std::env::var_os("BRIGADIER_CONNECT_TEST_LINK").unwrap())
        .status()
        .unwrap();
    assert!(status.success(), "the link catcher failed");
    let until = Instant::now() + Duration::from_secs(10);
    while !root.join("opened").exists() {
        assert!(
            Instant::now() < until,
            "Brigadier did not open the sign-in link"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mode = fs::read_to_string(root.join("login-mode")).unwrap_or_default();
    match mode.as_str() {
        "quit" => return ExitCode::FAILURE,
        "wait" => {
            std::thread::sleep(Duration::from_secs(60));
            return ExitCode::FAILURE;
        }
        _ => {}
    }
    fs::write(root.join("signed-in"), "yes").unwrap();
    if provider == "codex" {
        eprintln!("Successfully logged in");
    } else {
        println!("Login successful.");
    }
    if mode == "stay" {
        std::thread::sleep(Duration::from_secs(60));
    }
    ExitCode::SUCCESS
}
