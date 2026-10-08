use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;

/// A scratch folder of the test's own, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let dir = std::env::temp_dir().join(format!("brigadier-trust-{}", &id[..12]));
        fs::create_dir_all(&dir).unwrap();
        Self(fs::canonicalize(dir).unwrap())
    }

    fn file(&self, name: &str, text: Option<&str>) -> PathBuf {
        let path = self.0.join(name);
        if let Some(text) = text {
            fs::write(&path, text).unwrap();
        }
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const FOLDER: &str = "/Users/me/code/app";

/// Trusts `FOLDER`, checks the result reads as trusted, undoes it, and returns the text in
/// between.
fn round_trip(cli: TrustCli, file: &Path, before: TrustBefore) -> String {
    assert_eq!(plan(cli, file, FOLDER).unwrap(), Some(before.clone()));
    assert_eq!(write(cli, file, FOLDER, &before).unwrap(), Written::Trusted);
    assert_eq!(plan(cli, file, FOLDER).unwrap(), None);
    let trusted = fs::read_to_string(file).unwrap();
    undo(cli, file, FOLDER, &before).unwrap();
    trusted
}

// A file as Claude writes it, with what JSON.stringify keeps as is: unicode, escapes, numbers
// serde_json would print otherwise.
const CLAUDE: &str = r#"{
  "numStartups": 12,
  "big": 1e+21,
  "ratio": 0.30000000000000004,
  "name": "Zoë \"z\" \u001f /",
  "projects": {
    "/Users/me/other": {
      "allowedTools": [],
      "hasTrustDialogAccepted": true
    }
  },
  "oauthAccount": {
    "emailAddress": "x"
  }
}"#;

#[test]
fn claude_new_entry_round_trips_byte_for_byte() {
    let scratch = Scratch::new();
    let file = scratch.file(".claude.json", Some(CLAUDE));
    let trusted = round_trip(TrustCli::Claude, &file, TrustBefore::NoEntry);
    assert_eq!(fs::read_to_string(&file).unwrap(), CLAUDE);
    let value: serde_json::Value = serde_json::from_str(&trusted).unwrap();
    assert_eq!(value["projects"][FOLDER]["hasTrustDialogAccepted"], true);
    assert_eq!(
        value["projects"]["/Users/me/other"]["allowedTools"],
        serde_json::json!([])
    );
    // Only an insertion: the rest is the same text.
    assert!(trusted.starts_with(&CLAUDE[..CLAUDE.find("\n  },\n  \"oauth").unwrap()]));
    assert!(trusted.ends_with(&CLAUDE[CLAUDE.find("\n  },\n  \"oauth").unwrap()..]));
}

#[test]
fn claude_existing_entry_without_the_key_and_with_false_round_trip() {
    let scratch = Scratch::new();
    let no_key = format!(
        "{{\n  \"projects\": {{\n    \"{FOLDER}\": {{\n      \"allowedTools\": [\"Bash\"]\n    }}\n  }}\n}}"
    );
    let file = scratch.file("a.json", Some(&no_key));
    round_trip(TrustCli::Claude, &file, TrustBefore::NoKey);
    assert_eq!(fs::read_to_string(&file).unwrap(), no_key);

    let refused = no_key.replace(
        "\"allowedTools\": [\"Bash\"]",
        "\"hasTrustDialogAccepted\": false",
    );
    let file = scratch.file("b.json", Some(&refused));
    let before = TrustBefore::Value {
        raw: "false".into(),
    };
    round_trip(TrustCli::Claude, &file, before);
    assert_eq!(fs::read_to_string(&file).unwrap(), refused);
}

#[test]
fn claude_without_a_file_or_projects_gets_one() {
    let scratch = Scratch::new();
    let file = scratch.file("none.json", None);
    let trusted = round_trip(TrustCli::Claude, &file, TrustBefore::NoEntry);
    let value: serde_json::Value = serde_json::from_str(&trusted).unwrap();
    assert_eq!(value["projects"][FOLDER]["hasTrustDialogAccepted"], true);

    let file = scratch.file("bare.json", Some("{\n  \"theme\": \"dark\"\n}"));
    let trusted = round_trip(TrustCli::Claude, &file, TrustBefore::NoEntry);
    assert!(trusted.starts_with("{\n  \"theme\": \"dark\",\n  \"projects\": {"));
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(after, serde_json::json!({"theme": "dark", "projects": {}}));
}

#[test]
fn trust_the_user_set_is_never_recorded_or_removed() {
    let scratch = Scratch::new();
    let file = scratch.file(".claude.json", Some(CLAUDE));
    // Already trusted: nothing to write, nothing to record.
    assert_eq!(
        plan(TrustCli::Claude, &file, "/Users/me/other").unwrap(),
        None
    );
    // Trusted by Brigadier, then the user changed it: undo leaves their value.
    round_trip(TrustCli::Claude, &file, TrustBefore::NoEntry);
    assert_eq!(
        write(TrustCli::Claude, &file, FOLDER, &TrustBefore::NoEntry).unwrap(),
        Written::Trusted
    );
    let changed = fs::read_to_string(&file).unwrap().replacen(
        "\"hasTrustDialogAccepted\": true\n    }\n  },\n  \"oauth",
        "\"hasTrustDialogAccepted\": false\n    }\n  },\n  \"oauth",
        1,
    );
    fs::write(&file, &changed).unwrap();
    undo(TrustCli::Claude, &file, FOLDER, &TrustBefore::NoEntry).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), changed);
}

#[test]
fn a_write_whose_entry_changed_since_its_plan_does_nothing() {
    let scratch = Scratch::new();
    let file = scratch.file(".claude.json", Some(CLAUDE));
    assert_eq!(
        write(TrustCli::Claude, &file, FOLDER, &TrustBefore::NoKey).unwrap(),
        Written::Changed(TrustBefore::NoEntry)
    );
    assert_eq!(
        write(
            TrustCli::Claude,
            &file,
            "/Users/me/other",
            &TrustBefore::NoEntry
        )
        .unwrap(),
        Written::AlreadyTrusted
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), CLAUDE);
}

#[test]
fn folders_are_matched_by_their_decoded_key() {
    let scratch = Scratch::new();
    let folder = "/Users/me/Zoë's \"app\"";
    let text = "{\n  \"projects\": {\n    \"/Users/me/Zo\\u00eb's \\\"app\\\"\": {\n      \"hasTrustDialogAccepted\": false\n    }\n  }\n}";
    let file = scratch.file(".claude.json", Some(text));
    let before = TrustBefore::Value {
        raw: "false".into(),
    };
    assert_eq!(
        plan(TrustCli::Claude, &file, folder).unwrap(),
        Some(before.clone())
    );
    assert_eq!(
        write(TrustCli::Claude, &file, folder, &before).unwrap(),
        Written::Trusted
    );
    undo(TrustCli::Claude, &file, folder, &before).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn a_file_that_does_not_parse_is_left_alone() {
    let scratch = Scratch::new();
    let file = scratch.file(".claude.json", Some("{\"projects\": {"));
    assert!(plan(TrustCli::Claude, &file, FOLDER).is_err());
    assert!(write(TrustCli::Claude, &file, FOLDER, &TrustBefore::NoEntry).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "{\"projects\": {");
    let file = scratch.file("config.toml", Some("[projects\n"));
    assert!(write(TrustCli::Codex, &file, FOLDER, &TrustBefore::NoEntry).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "[projects\n");
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_is_written_where_it_points_with_its_mode() {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new();
    fs::create_dir_all(scratch.0.join("dotfiles")).unwrap();
    let real = scratch.file("dotfiles/claude.json", Some(CLAUDE));
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    let link = scratch.0.join(".claude.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        write(TrustCli::Claude, &link, FOLDER, &TrustBefore::NoEntry).unwrap(),
        Written::Trusted
    );
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(plan(TrustCli::Claude, &real, FOLDER).unwrap(), None);
    // No temporary file or lock is left.
    let mut left: Vec<String> = fs::read_dir(&scratch.0)
        .unwrap()
        .chain(fs::read_dir(scratch.0.join("dotfiles")).unwrap())
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, vec![".claude.json", "claude.json", "dotfiles"]);
}

#[test]
fn claude_writes_wait_for_its_lock_and_take_over_a_stale_one() {
    let scratch = Scratch::new();
    let file = scratch.file(".claude.json", Some(CLAUDE));
    let lock = scratch.0.join(".claude.json.lock");
    fs::create_dir(&lock).unwrap();
    // Held (fresh): the write gives up and changes nothing.
    assert!(write(TrustCli::Claude, &file, FOLDER, &TrustBefore::NoEntry).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), CLAUDE);
    assert!(lock.is_dir());
    // Stale (its holder died): taken over.
    let old = SystemTime::now() - Duration::from_secs(30);
    fs::File::open(&lock).unwrap().set_modified(old).unwrap();
    assert_eq!(
        write(TrustCli::Claude, &file, FOLDER, &TrustBefore::NoEntry).unwrap(),
        Written::Trusted
    );
    assert!(!lock.exists());
}

const CODEX: &str = r#"# my settings
model = "gpt-5.5"   # pinned

[tui]
screen_reader_detection_done = true

[projects."/Users/me/other"]
trust_level = "trusted"

[mcp_servers.docs]
command = "docs"
args = ["--port", "1"]
"#;

#[test]
fn codex_new_entry_round_trips_byte_for_byte() {
    let scratch = Scratch::new();
    let file = scratch.file("config.toml", Some(CODEX));
    let trusted = round_trip(TrustCli::Codex, &file, TrustBefore::NoEntry);
    assert_eq!(fs::read_to_string(&file).unwrap(), CODEX);
    assert_eq!(
        trusted,
        format!("{CODEX}\n[projects.\"{FOLDER}\"]\ntrust_level = \"trusted\"\n")
    );
    // Removed while another table follows it (Codex added one after).
    fs::write(&file, format!("{trusted}\n[notice]\nhide = true\n")).unwrap();
    undo(TrustCli::Codex, &file, FOLDER, &TrustBefore::NoEntry).unwrap();
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!("{CODEX}\n[notice]\nhide = true\n")
    );
}

#[test]
fn codex_entries_without_the_key_or_untrusted_round_trip() {
    let scratch = Scratch::new();
    let no_key = format!("{CODEX}\n[projects.\"{FOLDER}\"]\nnote = 1\n");
    let file = scratch.file("a.toml", Some(&no_key));
    let trusted = round_trip(TrustCli::Codex, &file, TrustBefore::NoKey);
    assert!(trusted.ends_with(&format!(
        "[projects.\"{FOLDER}\"]\ntrust_level = \"trusted\"\nnote = 1\n"
    )));
    assert_eq!(fs::read_to_string(&file).unwrap(), no_key);

    let untrusted = format!("{CODEX}\n[projects.\"{FOLDER}\"]\ntrust_level = \"untrusted\" # no\n");
    let file = scratch.file("b.toml", Some(&untrusted));
    round_trip(
        TrustCli::Codex,
        &file,
        TrustBefore::Value {
            raw: "\"untrusted\"".into(),
        },
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), untrusted);
}

#[test]
fn codex_without_a_file_gets_one_and_loses_it_again() {
    let scratch = Scratch::new();
    let file = scratch.file("config.toml", None);
    round_trip(TrustCli::Codex, &file, TrustBefore::NoEntry);
    assert_eq!(fs::read_to_string(&file).unwrap(), "");
}

#[test]
fn codex_layouts_a_splice_cant_edit_are_refused() {
    let scratch = Scratch::new();
    for text in [
        format!("projects = {{ \"{FOLDER}\" = {{ trust_level = \"untrusted\" }} }}\n"),
        format!("projects.\"{FOLDER}\".trust_level = \"untrusted\"\n"),
        format!("[projects]\n\"{FOLDER}\" = {{ trust_level = \"untrusted\" }}\n"),
    ] {
        let file = scratch.file("config.toml", Some(&text));
        assert!(plan(TrustCli::Codex, &file, FOLDER).is_err(), "{text}");
        assert!(write(TrustCli::Codex, &file, FOLDER, &TrustBefore::NoEntry).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
    }
    // Other folders written inline don't stop a new table.
    let text = "[projects]\n\"/elsewhere\" = { trust_level = \"trusted\" }\n";
    let file = scratch.file("config.toml", Some(text));
    round_trip(TrustCli::Codex, &file, TrustBefore::NoEntry);
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn the_files_follow_the_clis_environment() {
    use std::ffi::OsString;
    let env = |vars: &[(&str, &str)]| {
        crate::cli::CliEnv::from_vars(
            vars.iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        )
    };
    let home = env(&[("HOME", "/h")]);
    assert_eq!(
        TrustCli::Claude.file(&home),
        Some(PathBuf::from("/h/.claude.json"))
    );
    assert_eq!(
        TrustCli::Codex.file(&home),
        Some(PathBuf::from("/h/.codex/config.toml"))
    );
    let set = env(&[
        ("HOME", "/h"),
        ("CLAUDE_CONFIG_DIR", "/c"),
        ("CODEX_HOME", "/x"),
    ]);
    assert_eq!(
        TrustCli::Claude.file(&set),
        Some(PathBuf::from("/c/.claude.json"))
    );
    assert_eq!(
        TrustCli::Codex.file(&set),
        Some(PathBuf::from("/x/config.toml"))
    );
}
