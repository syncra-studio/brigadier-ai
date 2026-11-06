//! Claude's Keychain items go through `/usr/bin/security`, as Claude Code's own do: an item
//! written by another program loses the `apple-tool:` partition, and then every read Claude Code
//! makes through `security` prompts. Items that already lost it are put right once per daemon run
//! ([`repair`]), and until then stay on the framework, which reads them without a prompt.
//!
//! `security` has no timeout here on purpose: like the framework call it replaces, it only waits
//! when macOS is asking the user something, and the store lock's heartbeat keeps running meanwhile.
//! Its stderr is dropped and its output never reaches an error, so errors hold no credential data.

use std::collections::BTreeSet;

/// `security -i` cuts lines near 4096 characters and stores what's left. 2099 was seen working.
pub(super) const STDIN_LINE_LIMIT: usize = 2048;

/// What `security` exits with: the low byte of the Keychain status.
pub(super) const EXIT_NOT_FOUND: i32 = 44; // errSecItemNotFound, -25300

pub(super) fn is_claude(service: &str) -> bool {
    service.starts_with("Claude Code")
}

/// Whether `security` exited because the user said no to macOS's prompt.
pub(super) fn refused(code: Option<i32>) -> bool {
    matches!(code, Some(128 | 51)) // errSecUserCanceled -128, errSecAuthFailed -25293
}

pub(super) fn to_hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `security` arguments that store `hex`, creating the item or updating it in place.
pub(super) fn add_args(service: &str, account: &str, hex: &str) -> Vec<String> {
    [
        "add-generic-password",
        "-U",
        "-a",
        account,
        "-s",
        service,
        "-X",
        hex,
    ]
    .map(str::to_owned)
    .into()
}

/// The same command as a line for `security -i`, keeping the value out of `ps`; `None` when it
/// wouldn't fit under `limit`, or a name would need quoting rules `security` doesn't document.
pub(super) fn stdin_line(service: &str, account: &str, hex: &str, limit: usize) -> Option<String> {
    let plain = |name: &str| !name.contains(['"', '\\', '\n']);
    let line = format!("add-generic-password -U -a \"{account}\" -s \"{service}\" -X \"{hex}\"\n");
    (plain(service) && plain(account) && line.len() <= limit).then_some(line)
}

/// A password as `find-generic-password -w` printed it. It prints hex for anything with a byte
/// outside printable ASCII. Only Claude's JSON comes through here, which is never all hex digits.
pub(super) fn decode_password(stdout: &[u8]) -> Vec<u8> {
    let text = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    if text.is_empty() || !text.len().is_multiple_of(2) || !text.iter().all(u8::is_ascii_hexdigit) {
        return text.to_vec();
    }
    let digit = |byte: u8| (byte as char).to_digit(16).unwrap_or(0) as u8;
    text.chunks(2)
        .map(|pair| digit(pair[0]) << 4 | digit(pair[1]))
        .collect()
}

/// The Claude items (service, account) in a `security dump-keychain -a` whose partitions are
/// listed and lack `apple-tool:`. Reads attributes only; that command prints no secrets.
pub(super) fn broken_items(dump: &str) -> BTreeSet<(String, String)> {
    let mut broken = BTreeSet::new();
    let (mut service, mut account, mut partitions, mut listed) = (None, None, false, false);
    let mut finish = |service: &mut Option<String>, account: &mut Option<String>, bad: bool| {
        if let (Some(service), Some(account)) = (service.take(), account.take())
            && bad
            && is_claude(&service)
        {
            broken.insert((service, account));
        }
    };
    let mut bad = false;
    for line in dump.lines() {
        let line = line.trim();
        if line.starts_with("keychain: ") {
            finish(&mut service, &mut account, bad);
            (partitions, listed, bad) = (false, false, false);
        } else if let Some(name) = quoted(line, "\"svce\"<blob>=") {
            service = Some(name);
        } else if let Some(name) = quoted(line, "\"acct\"<blob>=") {
            account = Some(name);
        } else if line.starts_with("authorizations") {
            partitions = line.ends_with(" partition_id");
        } else if partitions
            && !listed
            && let Some(ids) = line.strip_prefix("description: ")
        {
            listed = true;
            bad = !ids.contains("apple-tool:");
        }
    }
    finish(&mut service, &mut account, bad);
    broken
}

fn quoted(line: &str, prefix: &str) -> Option<String> {
    let value = line.strip_prefix(prefix)?.strip_prefix('"')?;
    Some(value.strip_suffix('"')?.to_owned())
}

/// Where a repair keeps the old value while the item is gone, so a crash can't lose the login.
pub(super) fn backup_service(service: &str) -> String {
    format!("Brigadier repair of {service}")
}

#[cfg(all(not(test), target_os = "macos"))]
pub(super) use system::*;

#[cfg(all(not(test), target_os = "macos"))]
mod system {
    use std::io::{self, Write};
    use std::process::{Command, Output, Stdio};
    use std::sync::Mutex;

    use security_framework::passwords;

    use super::super::Blocked;
    use super::*;

    /// A Keychain error in plain words; never holds credential data. The user saying no to
    /// macOS's prompt (`errSecUserCanceled`, `errSecAuthFailed`) is told apart: see [`Blocked`].
    pub(in super::super) fn keychain_error(action: &str, code: i32) -> io::Error {
        if matches!(code, -128 | -25293) {
            io::Error::other(Blocked::KeychainRefused)
        } else {
            io::Error::other(format!("MCP Keychain {action} failed ({code})"))
        }
    }

    pub(in super::super) fn framework_get(
        service: &str,
        account: &str,
    ) -> io::Result<Option<Vec<u8>>> {
        match passwords::get_generic_password(service, account) {
            Ok(data) => Ok(Some(data)),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(error) => Err(keychain_error("read", error.code())),
        }
    }

    pub(in super::super) fn framework_set(
        service: &str,
        account: &str,
        data: &[u8],
    ) -> io::Result<()> {
        passwords::set_generic_password(service, account, data)
            .map_err(|error| keychain_error("write", error.code()))
    }

    fn framework_delete(service: &str, account: &str) -> io::Result<()> {
        match passwords::delete_generic_password(service, account) {
            Err(error) if error.code() != -25300 => Err(keychain_error("delete", error.code())),
            _ => Ok(()),
        }
    }

    fn security(args: &[String], stdin: Option<&str>) -> io::Result<Output> {
        let mut child = Command::new("/usr/bin/security")
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        if let (Some(line), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // The line fits a pipe's buffer, so this can't block. A failed write shows in the
            // read-back that follows every write.
            let _ = pipe.write_all(line.as_bytes());
        }
        child.wait_with_output()
    }

    fn security_failed(action: &str, code: Option<i32>) -> io::Error {
        if refused(code) {
            io::Error::other(Blocked::KeychainRefused)
        } else {
            let code = code.map_or_else(|| "signal".to_owned(), |code| code.to_string());
            io::Error::other(format!("MCP Keychain {action} failed ({code})"))
        }
    }

    fn security_get(service: &str, account: &str) -> io::Result<Option<Vec<u8>>> {
        let args = ["find-generic-password", "-a", account, "-s", service, "-w"].map(str::to_owned);
        let output = security(&args, None)?;
        match output.status.code() {
            Some(0) => Ok(Some(decode_password(&output.stdout))),
            Some(EXIT_NOT_FOUND) => Ok(None),
            code => Err(security_failed("read", code)),
        }
    }

    fn security_delete(service: &str, account: &str) -> io::Result<()> {
        let args = ["delete-generic-password", "-a", account, "-s", service].map(str::to_owned);
        match security(&args, None)?.status.code() {
            Some(0 | EXIT_NOT_FOUND) => Ok(()),
            code => Err(security_failed("delete", code)),
        }
    }

    /// Stores `data` through `security`, then reads it back. Anything else than the whole value
    /// puts `before` back: a truncated login is the worst way this can go.
    pub(in super::super) fn write_verified(
        service: &str,
        account: &str,
        data: &[u8],
        before: Option<&[u8]>,
        limit: usize,
    ) -> io::Result<()> {
        let hex = to_hex(data);
        let output = match stdin_line(service, account, &hex, limit) {
            Some(line) => security(&["-i".to_owned()], Some(&line))?,
            None => security(&add_args(service, account, &hex), None)?,
        };
        if refused(output.status.code()) {
            return Err(security_failed("write", output.status.code()));
        }
        let after = security_get(service, account).ok();
        if after
            .as_ref()
            .is_some_and(|after| after.as_deref() == Some(data))
        {
            return Ok(());
        }
        if after
            .as_ref()
            .is_none_or(|after| after.as_deref() != before)
        {
            match before {
                Some(old) => {
                    let args = add_args(service, account, &to_hex(old));
                    security(&args, None)?;
                }
                None => security_delete(service, account)?,
            }
        }
        Err(io::Error::other(
            "MCP Keychain write wasn't stored whole; the item was left as it was",
        ))
    }

    #[derive(Default)]
    struct Seen {
        /// Items that lost `apple-tool:`, from the run's one dump: it takes seconds.
        broken: BTreeSet<(String, String)>,
        /// Items this run already looked at to repair.
        tried: BTreeSet<(String, String)>,
    }
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

    fn with_seen<T>(use_it: impl FnOnce(&mut Seen) -> T) -> T {
        let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
        use_it(seen.get_or_insert_with(|| {
            // A dump that failed reads as nothing broken: at worst a prompt, as before this.
            let dump = security(&["dump-keychain".to_owned(), "-a".to_owned()], None);
            let broken = dump.map_or_else(
                |_| BTreeSet::new(),
                |dump| broken_items(&String::from_utf8_lossy(&dump.stdout)),
            );
            Seen {
                broken,
                ..Seen::default()
            }
        }))
    }

    fn is_broken(service: &str, account: &str) -> bool {
        with_seen(|seen| {
            seen.broken
                .contains(&(service.to_owned(), account.to_owned()))
        })
    }

    pub(in super::super) fn get(service: &str, account: &str) -> io::Result<Option<Vec<u8>>> {
        if is_broken(service, account) {
            framework_get(service, account)
        } else {
            security_get(service, account)
        }
    }

    pub(in super::super) fn set(service: &str, account: &str, data: &[u8]) -> io::Result<()> {
        if is_broken(service, account) {
            // Only after a failed repair: `security` would prompt, and the item stays as it is.
            return framework_set(service, account, data);
        }
        let before = security_get(service, account)?;
        write_verified(service, account, data, before.as_deref(), STDIN_LINE_LIMIT)
    }

    /// Once per item per run, under the store's lock: finishes a repair a crash interrupted, then
    /// remakes an item that lost `apple-tool:`, without a prompt. Never for the main home's item.
    pub(in super::super) fn repair(service: &str, account: &str) -> io::Result<()> {
        let item = (service.to_owned(), account.to_owned());
        let first = with_seen(|seen| seen.tried.insert(item.clone()));
        if !first || !is_claude(service) {
            return Ok(());
        }
        let broken = is_broken(service, account);
        let backup = backup_service(service);
        if let Some(saved) = framework_get(&backup, account)? {
            if !broken && security_get(service, account)?.is_none() {
                write_verified(service, account, &saved, None, STDIN_LINE_LIMIT)?;
            }
            framework_delete(&backup, account)?;
        }
        if broken {
            remake(service, account, &backup)?;
            with_seen(|seen| seen.broken.remove(&item));
        }
        Ok(())
    }

    /// The old value is held in a second item from before the delete until the new item reads
    /// back whole. If remaking fails it goes back through the framework, broken but intact.
    fn remake(service: &str, account: &str, backup: &str) -> io::Result<()> {
        let Some(old) = framework_get(service, account)? else {
            return Ok(());
        };
        framework_set(backup, account, &old)?;
        if let Err(error) = security_delete(service, account) {
            framework_delete(backup, account)?;
            return Err(error);
        }
        let added = write_verified(service, account, &old, None, STDIN_LINE_LIMIT);
        if added.is_err() {
            // Failing here keeps the backup, for the next run to put back.
            security_delete(service, account)?;
            framework_set(service, account, &old)?;
            if framework_get(service, account)?.as_deref() != Some(&old) {
                return Err(io::Error::other(
                    "MCP Keychain repair couldn't put the item back",
                ));
            }
        }
        framework_delete(backup, account)?;
        added
    }
}
