//! Wire provenance for daemon-only inputs. Text is never authority without registration.
use super::super::session_dir::write_atomic;
use crate::{Error, InjectionKind, Result};
use brigadier_providers::{InputPart, TurnInput};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

pub(super) const MARK: &str = "[Brigadier internal input ";
static DISK: Mutex<()> = Mutex::new(());
#[derive(Debug, Serialize, Deserialize)]
struct Registration {
    nonce: String,
    kinds: Vec<InjectionKind>,
    hash: String,
    bytes: usize,
    native: Option<String>,
}
pub(super) fn internal(kinds: &[InjectionKind]) -> bool {
    !kinds.is_empty()
        && kinds.iter().all(|kind| {
            !matches!(
                kind,
                InjectionKind::UserMessage
                    | InjectionKind::FollowUp
                    | InjectionKind::PeerMessage
                    | InjectionKind::Seam
                    | InjectionKind::Reseed
                    | InjectionKind::Instructions
            )
        })
}
fn read(record: &Path) -> Result<Vec<Registration>> {
    match std::fs::read(record.join("internal-inputs.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| Error::Invalid(format!("internal input provenance: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(Error::Invalid(e.to_string())),
    }
}
fn save(record: &Path, registrations: &[Registration]) -> Result<()> {
    std::fs::create_dir_all(record).map_err(|e| Error::Invalid(e.to_string()))?;
    write_atomic(
        &record.join("internal-inputs.json"),
        &serde_json::to_vec(registrations).map_err(|e| Error::Invalid(e.to_string()))?,
    )
}
/// Caller proves the absence of authored text. Shape and kind checks add a conservative guard.
pub(super) fn register(
    record: &Path,
    kinds: &[InjectionKind],
    input: &mut TurnInput,
) -> Result<()> {
    if !internal(kinds) || !input.files.is_empty() {
        return Ok(());
    }
    let [InputPart::Text(text)] = input.parts.as_mut_slice() else {
        return Ok(());
    };
    let nonce = uuid::Uuid::now_v7().to_string();
    let wire = format!("{MARK}{nonce}]\n{text}");
    let _guard = DISK.lock().unwrap();
    let mut registrations = read(record)?;
    registrations.push(Registration {
        nonce,
        kinds: kinds.to_vec(),
        hash: blake3::hash(wire.as_bytes()).to_hex().to_string(),
        bytes: wire.len(),
        native: None,
    });
    save(record, &registrations)?;
    *text = wire;
    Ok(())
}
/// A registered input binds once. Replays under that same native id are idempotent; a user's
/// later copy of the exact text has a different native id and is deliberately not hidden.
pub(super) fn bind(record: &Path, text: &str, native: &str) -> Result<bool> {
    let Some((nonce, _)) = text.strip_prefix(MARK).and_then(|s| s.split_once("]\n")) else {
        return Ok(false);
    };
    let _guard = DISK.lock().unwrap();
    let mut registrations = read(record)?;
    let Some(registered) = registrations
        .iter_mut()
        .find(|r| r.nonce == nonce && internal(&r.kinds))
    else {
        return Ok(false);
    };
    if registered.bytes != text.len()
        || registered.hash != blake3::hash(text.as_bytes()).to_hex().as_str()
    {
        return Ok(false);
    }
    if let Some(bound) = &registered.native {
        return Ok(bound == native);
    }
    registered.native = Some(native.to_owned());
    save(record, &registrations)?;
    Ok(true)
}
pub(super) fn bind_legacy(
    record: &Path,
    seq: i64,
    kind: InjectionKind,
    text: &str,
    native: &str,
) -> Result<bool> {
    if !internal(&[kind]) {
        return Ok(false);
    }
    let _guard = DISK.lock().unwrap();
    let mut registrations = read(record)?;
    let nonce = format!("legacy-{seq}");
    if let Some(registered) = registrations.iter().find(|r| r.nonce == nonce) {
        return Ok(
            registered.hash == blake3::hash(text.as_bytes()).to_hex().as_str()
                && registered.native.as_deref() == Some(native),
        );
    }
    registrations.push(Registration {
        nonce,
        kinds: vec![kind],
        hash: blake3::hash(text.as_bytes()).to_hex().to_string(),
        bytes: text.len(),
        native: Some(native.to_owned()),
    });
    save(record, &registrations)?;
    Ok(true)
}

pub(super) fn ids(record: &Path) -> HashSet<String> {
    let _guard = DISK.lock().unwrap();
    read(record)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| internal(&r.kinds))
        .filter_map(|r| r.native)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generic_reminder_and_background_provenance_binds_once_and_survives_replay() {
        let dir =
            std::env::temp_dir().join(format!("internal-provenance-{}", uuid::Uuid::now_v7()));
        let mut input = TurnInput::text("Your background command finished · exit 0");
        register(
            &dir,
            &[InjectionKind::BackgroundRun, InjectionKind::Reminder],
            &mut input,
        )
        .unwrap();
        let InputPart::Text(text) = &input.parts[0] else {
            panic!("text");
        };
        assert!(bind(&dir, text, "native-session:injection").unwrap());
        assert!(bind(&dir, text, "native-session:injection").unwrap());
        assert!(!bind(&dir, text, "native-session:authored-copy").unwrap());
        assert!(
            !bind(
                &dir,
                &format!("{text} and real user content"),
                "native-session:mixed"
            )
            .unwrap()
        );
        assert!(ids(&dir).contains("native-session:injection"));
        assert!(!ids(&dir).contains("native-session:authored-copy"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn user_peer_mixed_and_unregistered_text_stays_visible() {
        let dir =
            std::env::temp_dir().join(format!("internal-provenance-{}", uuid::Uuid::now_v7()));
        for kind in [
            InjectionKind::UserMessage,
            InjectionKind::PeerMessage,
            InjectionKind::FollowUp,
            InjectionKind::Seam,
        ] {
            let mut input = TurnInput::text("An identical authored message");
            let original = input.clone();
            register(&dir, &[kind], &mut input).unwrap();
            assert_eq!(input, original);
        }
        let mut mixed = TurnInput {
            parts: vec![
                InputPart::Text("Authored".into()),
                InputPart::Text("Internal".into()),
            ],
            files: Vec::new(),
        };
        let original = mixed.clone();
        register(&dir, &[InjectionKind::Reminder], &mut mixed).unwrap();
        assert_eq!(mixed, original);
        assert!(!bind(&dir, "[Brigadier internal input invented]\nhello", "s:u").unwrap());
        assert!(ids(&dir).is_empty());
    }
}
