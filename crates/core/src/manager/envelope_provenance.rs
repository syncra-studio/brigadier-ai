//! Wire provenance for daemon-only inputs. Text is never authority without registration.
use super::super::session_dir::write_atomic;
use crate::{Error, InjectionKind, Result};
use brigadier_providers::session_file::Said;
use brigadier_providers::{InputPart, TurnInput};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

pub(crate) const MARK: &str = "[Brigadier internal input ";
static DISK: Mutex<()> = Mutex::new(());
#[derive(Debug, Serialize, Deserialize)]
struct Registration {
    nonce: String,
    kinds: Vec<InjectionKind>,
    hash: String,
    bytes: usize,
    native: Option<String>,
    /// A send noted with [`note_sent`]: the user's own words in it, as ranges of its
    /// non-whitespace characters (none: all of it is Brigadier's). Its hash and bytes are its
    /// text's with the whitespace left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<Vec<(usize, usize)>>,
    /// When it was sent (ms since the epoch): only a message the CLI took after binds to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sent_at: Option<i64>,
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
        user: None,
        sent_at: None,
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
        user: None,
        sent_at: None,
    });
    save(record, &registrations)?;
    Ok(true)
}

/// How long before Brigadier noted a send the CLI's word of taking it may be stamped: the
/// CLI's clock is the same machine's, its file's stamp a moment off.
const SENT_SLACK_MS: i64 = 5_000;

/// A text with its whitespace left out: a CLI may space what it keeps otherwise than it was
/// sent (Claude joins a message's text blocks with a newline).
fn solid(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// How many non-whitespace characters `text` has: where the next part of an input starts, in
/// the units [`note_sent`] takes.
pub(crate) fn solid_len(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// The words of an input as its CLI keeps them: its text parts, in order.
pub(crate) fn input_words(input: &TurnInput) -> String {
    input
        .parts
        .iter()
        .filter_map(|part| match part {
            InputPart::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Notes text Brigadier sends a CLI, before it sends it, so the thread knows the message the
/// CLI keeps of it for Brigadier's (D44, L12): `user`, the user's own words in it as ranges of
/// its non-whitespace characters (none: it is all Brigadier's). Nothing goes on the wire.
pub(super) fn note_sent(record: &Path, text: &str, user: &[(usize, usize)], at: i64) -> Result<()> {
    let solid = solid(text);
    if solid.is_empty() {
        return Ok(());
    }
    let _guard = DISK.lock().unwrap();
    let mut registrations = read(record)?;
    registrations.push(Registration {
        nonce: uuid::Uuid::now_v7().to_string(),
        kinds: Vec::new(),
        hash: blake3::hash(solid.as_bytes()).to_hex().to_string(),
        bytes: solid.len(),
        native: None,
        user: Some(user.to_vec()),
        sent_at: Some(at),
    });
    save(record, &registrations)
}

/// The CLI's message `native` (`<session>:<item>`) with `text`, taken at `at`: the user's
/// words in it when it is a send Brigadier noted ([`note_sent`]). It binds once, to the oldest
/// such send of the same words not bound yet and sent before it; read again, it is the same.
pub(super) fn bind_sent(
    record: &Path,
    text: &str,
    native: &str,
    at: i64,
) -> Result<Option<Vec<(usize, usize)>>> {
    let _guard = DISK.lock().unwrap();
    let mut registrations = read(record)?;
    if let Some(user) = registrations
        .iter()
        .find(|r| r.native.as_deref() == Some(native))
        .and_then(|r| r.user.clone())
    {
        return Ok(Some(user));
    }
    let solid = solid(text);
    let hash = blake3::hash(solid.as_bytes()).to_hex().to_string();
    let Some(registered) = registrations.iter_mut().find(|r| {
        r.native.is_none()
            && r.user.is_some()
            && r.bytes == solid.len()
            && r.hash == hash
            && r.sent_at.is_none_or(|sent| sent <= at + SENT_SLACK_MS)
    }) else {
        return Ok(None);
    };
    registered.native = Some(native.to_owned());
    let user = registered.user.clone();
    save(record, &registrations)?;
    Ok(user)
}

/// The CLI's messages bound to sends Brigadier noted, by `<session>:<item>`: the user's words
/// in each.
pub(super) fn sent_ids(record: &Path) -> std::collections::HashMap<String, Vec<(usize, usize)>> {
    let _guard = DISK.lock().unwrap();
    read(record)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| Some((r.native?, r.user?)))
        .collect()
}

/// Whether a send Brigadier noted since `since` is bound to no message of the CLI's yet.
pub(super) fn unbound_since(record: &Path, since: i64) -> bool {
    let _guard = DISK.lock().unwrap();
    read(record)
        .unwrap_or_default()
        .iter()
        .any(|r| r.native.is_none() && r.user.is_some() && r.sent_at.is_some_and(|at| at >= since))
}

/// Who wrote message `text` that Brigadier sent with the user's words at `user`
/// ([`note_sent`]). No ranges means Brigadier's input; an empty range preserves a user's
/// attachment-only message, with no authored text to display.
pub(crate) fn said(text: &str, user: &[(usize, usize)]) -> Said {
    let at: Vec<(usize, usize)> = text
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .map(|(index, c)| (index, c.len_utf8()))
        .collect();
    let covered: usize = user.iter().map(|&(from, to)| to.saturating_sub(from)).sum();
    if !at.is_empty() && covered >= at.len() {
        return Said::User;
    }
    let words: Vec<&str> = user
        .iter()
        .filter(|&&(from, to)| from < to && to <= at.len())
        .map(|&(from, to)| {
            let (end, len) = at[to - 1];
            text[at[from].0..end + len].trim()
        })
        .filter(|words| !words.is_empty())
        .collect();
    if user.is_empty() {
        Said::Brigadier
    } else {
        Said::Both {
            user: words.join("\n\n"),
        }
    }
}

/// The user's words in a send of `text` Brigadier noted, bound or not, without binding it.
#[cfg(test)]
pub(super) fn noted(record: &Path, text: &str) -> Option<Vec<(usize, usize)>> {
    let solid = solid(text);
    let hash = blake3::hash(solid.as_bytes()).to_hex().to_string();
    let _guard = DISK.lock().unwrap();
    read(record)
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.user.is_some() && r.bytes == solid.len() && r.hash == hash)
        .and_then(|r| r.user)
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
    fn a_noted_send_binds_once_to_the_message_the_cli_keeps_of_it_however_spaced() {
        let dir = std::env::temp_dir().join(format!("sent-provenance-{}", uuid::Uuid::now_v7()));
        // Brigadier's own words, sent twice: each copy the CLI keeps binds one send.
        let carry = "Carry on from the hand-off note.";
        note_sent(&dir, carry, &[], 1_000).unwrap();
        note_sent(&dir, carry, &[], 2_000).unwrap();
        // One the CLI kept before Brigadier sent it is someone else's.
        assert_eq!(
            bind_sent(&dir, carry, "s:early", 1_000 - SENT_SLACK_MS - 1).unwrap(),
            None
        );
        assert_eq!(
            bind_sent(&dir, "Carry on from\nthe hand-off note.", "s:one", 1_100).unwrap(),
            Some(vec![])
        );
        assert_eq!(
            bind_sent(&dir, carry, "s:one", 1_100).unwrap(),
            Some(vec![]),
            "read again, the same"
        );
        assert_eq!(
            bind_sent(&dir, carry, "s:two", 2_100).unwrap(),
            Some(vec![])
        );
        assert_eq!(
            bind_sent(&dir, carry, "s:three", 3_000).unwrap(),
            None,
            "both sends bound"
        );
        assert_eq!(said(carry, &[]), Said::Brigadier);
        // The user's words with a frame before them and a note after: only theirs show.
        let frame = "[queued message abcd1234 · judge it]\n";
        let words = "Also the docs, please.";
        let note = "\n\n[answer] The user picked B.";
        let from = solid_len(frame);
        let user = [(from, from + solid_len(words))];
        let sent = format!("{frame}{words}{note}");
        note_sent(&dir, &sent, &user, 4_000).unwrap();
        let kept = sent.replace("\n\n", "\n");
        let bound = bind_sent(&dir, &kept, "s:mixed", 4_000).unwrap().unwrap();
        assert_eq!(said(&kept, &bound), Said::Both { user: words.into() });
        // All the user's: theirs.
        note_sent(&dir, words, &[(0, solid_len(words))], 5_000).unwrap();
        let bound = bind_sent(&dir, words, "s:user", 5_000).unwrap().unwrap();
        assert_eq!(said(words, &bound), Said::User);
        let ids = sent_ids(&dir);
        assert_eq!(ids.len(), 4);
        assert!(!ids.contains_key("s:early") && !ids.contains_key("s:three"));
        // None of it is the daemon's internal input, which the thread drops.
        assert!(super::ids(&dir).is_empty());
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
