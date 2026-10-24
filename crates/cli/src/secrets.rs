//! An MCP server's env and header values, kept in the Keychain instead of any config file: the
//! CLIs' configs name `brigadier mcp run <server> -- <command>` (env) or `brigadier secret
//! headers <server>` as a headers helper, and these commands hand the values over at launch.
//! They run without the app, so a plain `claude` or `codex` in a terminal works too.
//!
//! - `brigadier secret set <server> <name>`: keeps the value read from standard input.
//! - `brigadier secret headers <server>`: prints the server's values as a JSON object.
//! - `brigadier secret remove <server> [<name>]`: forgets one value, or all of the server's.
//! - `brigadier mcp run <server> -- <command> [<args>…]`: runs the server with its values in
//!   its environment, and `${NAME}` in its arguments replaced by them.
//!
//! A server is `<plugin>/<server>`. Its values are one Keychain item, written by this command
//! only, so reading them back never asks the user.

use std::collections::BTreeMap;
use std::io::{Read, Write};

/// The Keychain service the items are kept under; the account is the server.
pub const SERVICE: &str = "Brigadier MCP secrets";

/// One server's values, by name.
pub type Values = BTreeMap<String, String>;

/// Where the values are kept.
pub trait Store {
    fn get(&self, server: &str) -> Result<Values, String>;
    /// Replaces the server's values; none left removes its item.
    fn put(&self, server: &str, values: &Values) -> Result<(), String>;
}

/// The login Keychain (macOS); elsewhere every call fails.
pub struct Keychain;

#[cfg(target_os = "macos")]
impl Store for Keychain {
    fn get(&self, server: &str) -> Result<Values, String> {
        use security_framework::passwords::get_generic_password;
        /// `errSecItemNotFound`.
        const NOT_FOUND: i32 = -25300;
        match get_generic_password(SERVICE, server) {
            Ok(data) => serde_json::from_slice(&data)
                .map_err(|_| format!("the Keychain item for {server} isn't readable")),
            Err(err) if err.code() == NOT_FOUND => Ok(Values::new()),
            Err(err) => Err(format!("the Keychain refused: {err}")),
        }
    }

    fn put(&self, server: &str, values: &Values) -> Result<(), String> {
        use security_framework::passwords::{delete_generic_password, set_generic_password};
        if values.is_empty() {
            return match delete_generic_password(SERVICE, server) {
                Err(err) if err.code() != -25300 => Err(format!("the Keychain refused: {err}")),
                _ => Ok(()),
            };
        }
        let data = serde_json::to_vec(values).map_err(|err| err.to_string())?;
        set_generic_password(SERVICE, server, &data)
            .map_err(|err| format!("the Keychain refused: {err}"))
    }
}

#[cfg(not(target_os = "macos"))]
impl Store for Keychain {
    fn get(&self, _: &str) -> Result<Values, String> {
        Err("secrets are kept in the macOS Keychain, which this system doesn't have".into())
    }

    fn put(&self, server: &str, _: &Values) -> Result<(), String> {
        self.get(server).map(|_| ())
    }
}

const USAGE: &str = "usage: brigadier secret set <server> <name> (value on standard input)\n       \
                     brigadier secret headers <server>\n       \
                     brigadier secret remove <server> [<name>]\n       \
                     brigadier mcp run <server> -- <command> [<args>...]";

/// Runs `words` when they are one of these commands; `None` for any other.
pub fn run_local(
    words: &[String],
    store: &dyn Store,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Option<u8> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["secret", "set", server, name] => set(store, server, name, stdin),
        ["secret", "headers", server] => headers(store, server, stdout),
        ["secret", "remove", server] => store.put(server, &Values::new()),
        ["secret", "remove", server, name] => remove(store, server, name),
        ["secret", ..] => Err(USAGE.to_owned()),
        ["mcp", "run", server, "--", command, args @ ..] => mcp_run(store, server, command, args)
            .map(|code| {
                let _ = stdout.flush();
                std::process::exit(code)
            }),
        ["mcp", ..] => Err(USAGE.to_owned()),
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(message) => {
            let _ = writeln!(stderr, "brigadier: {message}");
            1
        }
    })
}

fn check_server(server: &str) -> Result<(), String> {
    let ok = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match server.split_once('/') {
        Some((plugin, name)) if ok(plugin) && ok(name) => Ok(()),
        _ => Err(format!("`{server}` isn't a server: <plugin>/<server>")),
    }
}

fn set(store: &dyn Store, server: &str, name: &str, stdin: &mut dyn Read) -> Result<(), String> {
    check_server(server)?;
    if name.is_empty() || name.contains(['=', '\0', '\n']) {
        return Err(format!("`{name}` can't be a variable or header name"));
    }
    let mut value = String::new();
    stdin
        .read_to_string(&mut value)
        .map_err(|err| format!("reading the value: {err}"))?;
    let value = value
        .strip_suffix('\n')
        .map(|v| v.strip_suffix('\r').unwrap_or(v))
        .unwrap_or(&value);
    let mut values = store.get(server)?;
    values.insert(name.to_owned(), value.to_owned());
    store.put(server, &values)
}

fn remove(store: &dyn Store, server: &str, name: &str) -> Result<(), String> {
    check_server(server)?;
    let mut values = store.get(server)?;
    if values.remove(name).is_some() {
        store.put(server, &values)?;
    }
    Ok(())
}

fn headers(store: &dyn Store, server: &str, stdout: &mut dyn Write) -> Result<(), String> {
    check_server(server)?;
    let values = store.get(server)?;
    let text = serde_json::to_string(&values).map_err(|err| err.to_string())?;
    writeln!(stdout, "{text}").map_err(|err| err.to_string())
}

/// `arg` with each `${NAME}` that `values` has replaced by its value; any other is left.
pub fn expand(arg: &str, values: &Values) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) if values.contains_key(&after[..end]) => {
                out.push_str(&values[&after[..end]]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The server's command, with its values in its environment and arguments.
pub fn command(values: &Values, program: &str, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(expand(program, values));
    command
        .args(args.iter().map(|arg| expand(arg, values)))
        .envs(values);
    command
}

/// Runs the server in place of this process (Unix), or as a child whose exit code it returns.
fn mcp_run(store: &dyn Store, server: &str, program: &str, args: &[&str]) -> Result<i32, String> {
    check_server(server)?;
    let values = store.get(server)?;
    let mut command = command(&values, program, args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec();
        Err(format!("couldn't run {program}: {err}"))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|err| format!("couldn't run {program}: {err}"))?;
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, Values>>);

    impl Store for Memory {
        fn get(&self, server: &str) -> Result<Values, String> {
            Ok(self.0.borrow().get(server).cloned().unwrap_or_default())
        }
        fn put(&self, server: &str, values: &Values) -> Result<(), String> {
            let mut all = self.0.borrow_mut();
            if values.is_empty() {
                all.remove(server);
            } else {
                all.insert(server.to_owned(), values.clone());
            }
            Ok(())
        }
    }

    fn call(store: &Memory, words: &[&str], stdin: &str) -> (Option<u8>, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_local(&words, store, &mut stdin.as_bytes(), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn values_go_in_one_at_a_time_and_come_out_as_headers() {
        let store = Memory::default();
        assert_eq!(
            call(
                &store,
                &["secret", "set", "lin/api", "Authorization"],
                "Bearer abc\n"
            )
            .0,
            Some(0)
        );
        call(&store, &["secret", "set", "lin/api", "X-Team"], "core");
        let (code, out, _) = call(&store, &["secret", "headers", "lin/api"], "");
        assert_eq!(code, Some(0));
        assert_eq!(
            out,
            "{\"Authorization\":\"Bearer abc\",\"X-Team\":\"core\"}\n"
        );
        call(&store, &["secret", "remove", "lin/api", "X-Team"], "");
        assert_eq!(
            call(&store, &["secret", "headers", "lin/api"], "").1,
            "{\"Authorization\":\"Bearer abc\"}\n"
        );
        call(&store, &["secret", "remove", "lin/api"], "");
        assert!(store.0.borrow().is_empty());
        assert_eq!(
            call(&store, &["secret", "headers", "lin/api"], "").1,
            "{}\n"
        );
    }

    #[test]
    fn a_bad_server_or_command_is_refused_and_others_pass_through() {
        let store = Memory::default();
        let (code, _, err) = call(&store, &["secret", "set", "nope", "A"], "x");
        assert_eq!(code, Some(1));
        assert!(err.contains("<plugin>/<server>"), "{err}");
        assert_eq!(call(&store, &["secret", "list"], "").0, Some(1));
        assert_eq!(call(&store, &["mcp", "run", "a/b"], "").0, Some(1));
        assert_eq!(call(&store, &["sessions"], "").0, None);
        assert!(store.0.borrow().is_empty());
    }

    #[test]
    fn a_server_runs_with_its_values_in_env_and_args() {
        let values = Values::from([
            ("TOKEN".to_owned(), "s3cret".to_owned()),
            ("ORG".to_owned(), "acme".to_owned()),
        ]);
        assert_eq!(expand("--org=${ORG}", &values), "--org=acme");
        assert_eq!(expand("${MISSING}-${ORG}", &values), "${MISSING}-acme");
        assert_eq!(expand("${", &values), "${");
        let command = command(&values, "/bin/server", &["--token", "${TOKEN}"]);
        assert_eq!(command.get_program(), "/bin/server");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--token", "s3cret"]
        );
        let env: Vec<_> = command.get_envs().collect();
        assert!(env.contains(&("TOKEN".as_ref(), Some("s3cret".as_ref()))));
    }

    /// The real command, end to end, with a value in a throwaway store.
    #[cfg(unix)]
    #[test]
    fn mcp_run_hands_the_values_to_the_server() {
        let values = Values::from([("GREETING".to_owned(), "hi there".to_owned())]);
        let output = command(
            &values,
            "/bin/sh",
            &["-c", "echo \"$GREETING|$1\"", "sh", "${GREETING}"],
        )
        .output()
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "hi there|hi there\n"
        );
    }
}
