//! Best-effort embedded-driver process state file.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

pub const STATE_DIR_ENV: &str = "BRIGADIER_CU_STATE_DIR";
pub const STATE_WRITER_PID_ARG: &str = "_state_writer_pid";
pub const STATE_WRITER_START_SECONDS_ARG: &str = "_state_writer_start_seconds";
pub const STATE_WRITER_START_MICROSECONDS_ARG: &str = "_state_writer_start_microseconds";
pub const STATE_OWNER_PID_ARG: &str = "_state_owner_pid";
const UNSIGNED_SCHEMA_VERSION: u8 = 3;
const AUTHENTICATED_SCHEMA_VERSION: u8 = 4;
const AUTHENTICATION_DOMAIN: &[u8] = b"brigadier-computer-use-state-v1\0";

/// Cross-platform fallback process-name resolver. Platform registries may
/// replace this with a native resolver when they have one.
pub fn resolve_process_name(pid: i64) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    #[cfg(unix)]
    {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()?;
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if raw.is_empty() {
            return None;
        }
        Some(
            Path::new(&raw)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&raw)
                .to_owned(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriverProcessState {
    pub driver_pid: u32,
    /// Kernel-authenticated process that sent the action to a long-running daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_pid: Option<u32>,
    /// Kernel-observed start time for `writer_pid`; together these identify one
    /// process generation instead of trusting a reusable numeric pid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_start_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_start_microseconds: Option<i64>,
    pub session: Option<String>,
    pub target_app: Option<String>,
    pub target_pid: Option<i64>,
    pub target_window_id: Option<u64>,
    pub last_action_at: String,
    pub schema: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_authentication_code: Option<String>,
}

impl DriverProcessState {
    fn for_action(driver_pid: u32, args: &serde_json::Value, target_app: Option<String>) -> Self {
        let writer_identity = args
            .get(STATE_WRITER_PID_ARG)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 1)
            .and_then(|process_id| {
                let start_seconds = args
                    .get(STATE_WRITER_START_SECONDS_ARG)?
                    .as_i64()
                    .filter(|value| *value > 0)?;
                let start_microseconds = args
                    .get(STATE_WRITER_START_MICROSECONDS_ARG)?
                    .as_i64()
                    .filter(|value| (0..1_000_000).contains(value))?;
                Some((process_id, start_seconds, start_microseconds))
            });
        Self {
            driver_pid,
            writer_pid: writer_identity.map(|identity| identity.0),
            writer_start_seconds: writer_identity.map(|identity| identity.1),
            writer_start_microseconds: writer_identity.map(|identity| identity.2),
            session: session_for_action(args, crate::embedded_default_session_id()),
            target_app,
            target_pid: args.get("pid").and_then(|value| value.as_i64()),
            target_window_id: args.get("window_id").and_then(|value| value.as_u64()),
            last_action_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned()),
            schema: UNSIGNED_SCHEMA_VERSION,
            state_authentication_code: None,
        }
    }

    /// Stable length-delimited bytes authenticated by the brigadier-owned helper.
    pub fn authentication_message(&self) -> Vec<u8> {
        let mut message = Vec::with_capacity(256);
        message.extend_from_slice(AUTHENTICATION_DOMAIN);
        Self::append_integer(&mut message, self.driver_pid);
        Self::append_optional_integer(&mut message, self.writer_pid);
        Self::append_optional_integer(&mut message, self.writer_start_seconds);
        Self::append_optional_integer(&mut message, self.writer_start_microseconds);
        Self::append_optional_string(&mut message, self.session.as_deref());
        Self::append_optional_string(&mut message, self.target_app.as_deref());
        Self::append_optional_integer(&mut message, self.target_pid);
        Self::append_optional_integer(&mut message, self.target_window_id);
        Self::append_string(&mut message, &self.last_action_at);
        Self::append_integer(&mut message, self.schema);
        message
    }

    fn authenticate(&mut self, key: &[u8]) -> std::io::Result<()> {
        self.schema = AUTHENTICATED_SCHEMA_VERSION;
        self.state_authentication_code = None;
        let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid state authentication key",
            )
        })?;
        mac.update(&self.authentication_message());
        let code = mac.finalize().into_bytes();
        self.state_authentication_code = Some(
            code.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        );
        Ok(())
    }

    #[cfg(test)]
    fn has_valid_authentication_code(&self, key: &[u8]) -> bool {
        let Some(provided) = self
            .state_authentication_code
            .as_deref()
            .and_then(Self::decode_hex)
        else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
            return false;
        };
        mac.update(&self.authentication_message());
        mac.verify_slice(&provided).is_ok()
    }

    fn append_integer(message: &mut Vec<u8>, value: impl std::fmt::Display) {
        message.extend_from_slice(value.to_string().as_bytes());
        message.push(0);
    }

    fn append_optional_integer(message: &mut Vec<u8>, value: Option<impl std::fmt::Display>) {
        match value {
            Some(value) => Self::append_integer(message, value),
            None => message.extend_from_slice(b"-\0"),
        }
    }

    fn append_string(message: &mut Vec<u8>, value: &str) {
        message.extend_from_slice(value.len().to_string().as_bytes());
        message.push(b':');
        message.extend_from_slice(value.as_bytes());
        message.push(0);
    }

    fn append_optional_string(message: &mut Vec<u8>, value: Option<&str>) {
        match value {
            Some(value) => Self::append_string(message, value),
            None => message.extend_from_slice(b"-\0"),
        }
    }

    #[cfg(test)]
    fn decode_hex(value: &str) -> Option<Vec<u8>> {
        if !value.len().is_multiple_of(2) {
            return None;
        }
        (0..value.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
            .collect()
    }
}

fn session_for_action(args: &serde_json::Value, embedded_default: Option<&str>) -> Option<String> {
    args.get(crate::HOST_SESSION_ARG)
        .or_else(|| args.get("session"))
        .or_else(|| args.get("_session_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(embedded_default)
        .map(str::to_owned)
}

/// Session-scoped state files for the current driver process. Writes are always
/// performed through a same-directory temporary file followed by `rename`.
pub struct StateFile {
    dir: PathBuf,
    driver_pid: u32,
    temp_counter: AtomicU64,
    authentication_key: RwLock<Option<Vec<u8>>>,
}

impl StateFile {
    pub fn from_env() -> Option<Self> {
        std::env::var_os(STATE_DIR_ENV).map(|dir| Self::new(PathBuf::from(dir), std::process::id()))
    }

    pub fn new(dir: PathBuf, driver_pid: u32) -> Self {
        Self {
            dir,
            driver_pid,
            temp_counter: AtomicU64::new(0),
            authentication_key: RwLock::new(None),
        }
    }

    /// Replaces the in-memory state signing key. The key is never read from an
    /// environment variable or file inherited by automation clients.
    pub fn set_authentication_key(&self, key: Vec<u8>) -> bool {
        if key.len() != 32 {
            return false;
        }
        let Ok(mut current) = self.authentication_key.write() else {
            return false;
        };
        *current = Some(key);
        true
    }

    pub fn path(&self) -> PathBuf {
        self.path_for_session(None)
    }

    pub(crate) fn path_for_session(&self, session: Option<&str>) -> PathBuf {
        let file_name = match session {
            Some(session) => {
                let mut hasher = DefaultHasher::new();
                session.hash(&mut hasher);
                format!("{}-{:016x}.json", self.driver_pid, hasher.finish())
            }
            None => format!("{}.json", self.driver_pid),
        };
        self.dir.join(file_name)
    }

    pub fn update(
        &self,
        args: &serde_json::Value,
        target_app: Option<String>,
    ) -> std::io::Result<()> {
        ensure_private_dir(&self.dir)?;
        let mut state = DriverProcessState::for_action(self.driver_pid, args, target_app);
        let authentication_key = self
            .authentication_key
            .read()
            .map_err(|_| std::io::Error::other("state authentication key lock poisoned"))?
            .clone();
        if let Some(key) = authentication_key {
            state.authenticate(&key)?;
        }
        let state_path = self.path_for_session(state.session.as_deref());
        let body = serde_json::to_vec(&state)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let sequence = self.temp_counter.fetch_add(1, Ordering::Relaxed);
        let state_file_name = state_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid state filename")
            })?;
        let temp_path = self.dir.join(format!(".{state_file_name}.tmp-{sequence}"));

        let result = (|| {
            use std::io::Write;
            let mut file = create_private_temp_file(&temp_path)?;
            file.write_all(&body)?;
            file.sync_all()?;
            std::fs::rename(&temp_path, state_path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result
    }

    pub fn remove(&self) -> std::io::Result<()> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let anonymous_file_name = format!("{}.json", self.driver_pid);
        let session_file_prefix = format!("{}-", self.driver_pid);
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let is_owned_state = file_name == anonymous_file_name
                || (file_name.starts_with(&session_file_prefix) && file_name.ends_with(".json"));
            if !is_owned_state {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(()) => {}
            }
        }
        Ok(())
    }

    /// Removes the state owned by one ended logical session while preserving
    /// concurrently active sessions served by the same daemon.
    pub fn remove_session(&self, session: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.path_for_session(Some(session))) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

impl Drop for StateFile {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!("[brigadier-cu] warning: failed to remove state file: {error}");
        }
    }
}

pub(crate) fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The host may supply an existing state directory. Re-apply the privacy
        // invariant on every write rather than trusting its prior mode.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn create_private_temp_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_embedded_action_uses_the_cursor_session_default() {
        assert_eq!(
            session_for_action(&serde_json::json!({"pid": 10}), Some("brigadier-codex-42")),
            Some("brigadier-codex-42".to_owned())
        );
        assert_eq!(
            session_for_action(
                &serde_json::json!({"session": " explicit ", "_session_id": "proxy"}),
                Some("embedded-default"),
            ),
            Some("explicit".to_owned()),
            "an explicit session must retain precedence over proxy/default identity"
        );
    }

    #[test]
    fn managed_host_session_survives_proxy_generation_turnover() {
        let args = serde_json::json!({
            "_host_session": "brigadier-surface-a",
            "session": "brigadier-surface-a-mcp-4242-99",
            "_session_id": "brigadier-surface-a-mcp-4242-99",
            "pid": 10,
        });

        assert_eq!(
            session_for_action(&args, Some("embedded-default")),
            Some("brigadier-surface-a".to_owned()),
            "durable host state must use the surface identity, not a short-lived proxy generation"
        );
    }

    #[test]
    fn update_atomically_replaces_the_process_file() {
        let dir = tempfile::tempdir().unwrap();
        let writer = StateFile::new(dir.path().to_owned(), 4242);
        writer
            .update(
                &serde_json::json!({"session":"surface-a","pid":10,"window_id":20}),
                Some("Notes".to_owned()),
            )
            .unwrap();
        writer
            .update(
                &serde_json::json!({
                    "session":"surface-a",
                    "pid":11,
                    "window_id":21,
                    "_state_writer_pid": 3131,
                    "_state_writer_start_seconds": 1_700_000_000,
                    "_state_writer_start_microseconds": 123_456,
                }),
                Some("Safari".to_owned()),
            )
            .unwrap();

        let state: DriverProcessState = serde_json::from_slice(
            &std::fs::read(writer.path_for_session(Some("surface-a"))).unwrap(),
        )
        .unwrap();
        assert_eq!(state.driver_pid, 4242);
        assert_eq!(state.writer_pid, Some(3131));
        assert_eq!(state.writer_start_seconds, Some(1_700_000_000));
        assert_eq!(state.writer_start_microseconds, Some(123_456));
        assert_eq!(state.session.as_deref(), Some("surface-a"));
        assert_eq!(state.target_app.as_deref(), Some("Safari"));
        assert_eq!(state.target_pid, Some(11));
        assert_eq!(state.target_window_id, Some(21));
        assert_eq!(state.schema, 3);
        assert!(time::OffsetDateTime::parse(
            &state.last_action_at,
            &time::format_description::well_known::Rfc3339,
        )
        .is_ok());
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
                .count(),
            0,
            "rename must leave no temporary file behind",
        );
    }

    #[test]
    fn configured_key_authenticates_state_and_detects_forgery() {
        let dir = tempfile::tempdir().unwrap();
        let writer = StateFile::new(dir.path().to_owned(), 4242);
        let key = vec![0x5a; 32];
        assert!(writer.set_authentication_key(key.clone()));
        assert!(!writer.set_authentication_key(vec![0x5a; 31]));
        writer
            .update(
                &serde_json::json!({
                    "session": "surface-a",
                    "pid": 10,
                    "window_id": 20,
                    "_state_writer_pid": 3131,
                    "_state_writer_start_seconds": 1_700_000_000,
                    "_state_writer_start_microseconds": 123_456,
                }),
                Some("Notes".to_owned()),
            )
            .unwrap();

        let mut state: DriverProcessState = serde_json::from_slice(
            &std::fs::read(writer.path_for_session(Some("surface-a"))).unwrap(),
        )
        .unwrap();
        assert_eq!(state.schema, AUTHENTICATED_SCHEMA_VERSION);
        assert_eq!(
            state.state_authentication_code.as_deref().map(str::len),
            Some(64)
        );
        assert!(state.has_valid_authentication_code(&key));

        state.target_pid = Some(11);
        assert!(!state.has_valid_authentication_code(&key));
    }

    #[test]
    fn authenticated_state_contract_matches_brigadier() {
        let key = vec![0x5a; 32];
        let mut state = DriverProcessState {
            driver_pid: 71_790,
            writer_pid: Some(71_600),
            writer_start_seconds: Some(1_700_000_000),
            writer_start_microseconds: Some(123_456),
            session: None,
            target_app: Some("Calculator".to_owned()),
            target_pid: Some(71_241),
            target_window_id: Some(87_692),
            last_action_at: "2026-07-14T01:09:37.745752Z".to_owned(),
            schema: UNSIGNED_SCHEMA_VERSION,
            state_authentication_code: None,
        };
        state.authenticate(&key).unwrap();

        assert_eq!(
            state.state_authentication_code.as_deref(),
            Some("cf5fd50f1edef578d1403e078611e099ca00ada984955a5c615c1bad75702c82")
        );
        assert!(state.has_valid_authentication_code(&key));
    }

    #[test]
    fn updates_for_distinct_sessions_preserve_both_states() {
        let dir = tempfile::tempdir().unwrap();
        let writer = StateFile::new(dir.path().to_owned(), 4242);
        writer
            .update(
                &serde_json::json!({
                    "session": "surface-a",
                    "pid": 10,
                    "window_id": 20,
                    "_state_writer_pid": 3131,
                    "_state_writer_start_seconds": 1_700_000_000,
                    "_state_writer_start_microseconds": 123_456,
                }),
                Some("Notes".to_owned()),
            )
            .unwrap();
        writer
            .update(
                &serde_json::json!({
                    "session": "surface-b",
                    "pid": 11,
                    "window_id": 21,
                    "_state_writer_pid": 4141,
                    "_state_writer_start_seconds": 1_700_000_001,
                    "_state_writer_start_microseconds": 234_567,
                }),
                Some("Safari".to_owned()),
            )
            .unwrap();

        let mut states = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .map(|entry| {
                serde_json::from_slice::<DriverProcessState>(&std::fs::read(entry.path()).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        states.sort_by(|lhs, rhs| lhs.session.cmp(&rhs.session));

        assert_eq!(states.len(), 2);
        assert_eq!(states[0].session.as_deref(), Some("surface-a"));
        assert_eq!(states[0].writer_pid, Some(3131));
        assert_eq!(states[0].writer_start_seconds, Some(1_700_000_000));
        assert_eq!(states[0].writer_start_microseconds, Some(123_456));
        assert_eq!(states[1].session.as_deref(), Some("surface-b"));
        assert_eq!(states[1].writer_pid, Some(4141));
        assert_eq!(states[1].writer_start_seconds, Some(1_700_000_001));
        assert_eq!(states[1].writer_start_microseconds, Some(234_567));
    }

    #[test]
    fn session_teardown_removes_only_its_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let writer = StateFile::new(dir.path().to_owned(), 4242);
        for session in ["surface-a", "surface-b"] {
            writer
                .update(
                    &serde_json::json!({
                        "session": session,
                        "pid": 10,
                        "window_id": 20,
                        "_state_writer_pid": 3131,
                        "_state_writer_start_seconds": 1_700_000_000,
                        "_state_writer_start_microseconds": 123_456,
                    }),
                    Some("Notes".to_owned()),
                )
                .unwrap();
        }

        writer.remove_session("surface-a").unwrap();

        assert!(!writer.path_for_session(Some("surface-a")).exists());
        assert!(writer.path_for_session(Some("surface-b")).exists());
    }

    #[test]
    fn proxy_teardown_preserves_managed_host_activity_state() {
        let dir = tempfile::tempdir().unwrap();
        let writer = StateFile::new(dir.path().to_owned(), 4242);
        let host_session = "brigadier-surface-a";
        let proxy_session = "brigadier-surface-a-mcp-4242-99";
        writer
            .update(
                &serde_json::json!({
                    "_host_session": host_session,
                    "session": proxy_session,
                    "_session_id": proxy_session,
                    "pid": 10,
                    "window_id": 20,
                    "_state_writer_pid": 3131,
                    "_state_writer_start_seconds": 1_700_000_000,
                    "_state_writer_start_microseconds": 123_456,
                }),
                Some("Notes".to_owned()),
            )
            .unwrap();

        writer.remove_session(proxy_session).unwrap();

        assert!(
            writer.path_for_session(Some(host_session)).exists(),
            "the menu's last authenticated activity must outlive one MCP proxy process"
        );
    }

    #[test]
    fn malformed_state_directory_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("plain-file");
        std::fs::write(&not_a_dir, b"occupied").unwrap();
        let writer = StateFile::new(not_a_dir, 4343);
        assert!(writer
            .update(&serde_json::json!({"pid": 10}), None)
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn update_repairs_existing_directory_and_writes_private_state_file() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let state_dir = parent.path().join("state");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let writer = StateFile::new(state_dir.clone(), 4545);
        writer
            .update(&serde_json::json!({"session": "private"}), None)
            .unwrap();

        assert_eq!(
            std::fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(writer.path_for_session(Some("private")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn drop_removes_the_process_file_on_clean_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let first_path;
        let second_path;
        {
            let writer = StateFile::new(dir.path().to_owned(), 4444);
            writer
                .update(
                    &serde_json::json!({"session": "surface-a", "pid": 10}),
                    Some("Notes".to_owned()),
                )
                .unwrap();
            writer
                .update(
                    &serde_json::json!({"session": "surface-b", "pid": 11}),
                    Some("Safari".to_owned()),
                )
                .unwrap();
            first_path = writer.path_for_session(Some("surface-a"));
            second_path = writer.path_for_session(Some("surface-b"));
            assert!(first_path.exists());
            assert!(second_path.exists());
        }
        assert!(!first_path.exists());
        assert!(!second_path.exists());
    }
}
