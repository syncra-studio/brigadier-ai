//! Where the Library keeps an MCP server's env and header values: the Keychain, written by the
//! `brigadier` CLI itself (`brigadier secret set`), so that `brigadier mcp run` and `brigadier
//! secret headers`, which read them back when a CLI starts the server, are the item's own
//! program and never make macOS ask. No value is written to a file or a command line.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use brigadier_registry::Secret;
use brigadier_registry::library::SecretSink;

/// The Keychain through the `brigadier` CLI at `brigadier`; a server is `<plugin>/<server>`.
pub(crate) struct KeychainSecrets {
    brigadier: PathBuf,
}

/// The Library's [`SecretSink`], through the `brigadier` CLI at `brigadier`
/// (`SessionManager::brigadier_cli`). Not called yet: the Library's Add goes through it.
#[allow(dead_code)]
pub(crate) fn keychain_secret_sink(brigadier: PathBuf) -> KeychainSecrets {
    KeychainSecrets { brigadier }
}

impl KeychainSecrets {
    fn run(&self, args: &[&str], input: Option<&str>) -> Result<(), String> {
        let mut child = Command::new(&self.brigadier)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("couldn't run brigadier: {err}"))?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin
                .write_all(input.as_bytes())
                .map_err(|err| format!("couldn't hand brigadier the value: {err}"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|err| format!("brigadier failed: {err}"))?;
        if output.status.success() {
            return Ok(());
        }
        let words = String::from_utf8_lossy(&output.stderr);
        Err(words.trim().trim_start_matches("brigadier: ").to_owned())
    }

    /// Forgets every value kept for `server`.
    #[allow(dead_code)]
    pub(crate) fn forget(&self, server: &str) -> Result<(), String> {
        self.run(&["secret", "remove", server], None)
    }
}

impl SecretSink for KeychainSecrets {
    fn store(&self, server: &str, name: &str, value: &Secret) -> Result<(), String> {
        self.run(&["secret", "set", server, name], Some(value.expose()))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A stand-in `brigadier` that records what it was asked, value from its input included.
    #[cfg(unix)]
    #[test]
    fn a_value_goes_to_the_cli_on_its_input_never_its_command_line() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "brigadier-secret-sink-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("brigadier");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$*\" >> {log}\ncat >> {log}\n[ \"$3\" = bad/x ] && {{ echo 'brigadier: the Keychain refused' >&2; exit 1; }}\nexit 0\n",
                log = dir.join("log").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sink = keychain_secret_sink(fake);
        sink.store("lin/api", "TOKEN", &Secret::new("s3cret"))
            .unwrap();
        sink.forget("lin/api").unwrap();
        assert_eq!(
            sink.store("bad/x", "A", &Secret::new("v")),
            Err("the Keychain refused".to_owned())
        );
        let log = std::fs::read_to_string(dir.join("log")).unwrap();
        assert!(log.starts_with("secret set lin/api TOKEN\ns3cret"), "{log}");
        assert!(log.contains("secret remove lin/api\n"), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
