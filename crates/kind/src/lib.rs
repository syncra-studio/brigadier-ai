//! The id of a CLI Brigadier can drive (`"claude"`, `"codex"`, `"opencode"`, …).
//!
//! The set is open: each adapter names its own id and registers itself in
//! `brigadier_providers::adapters`; nothing here lists them. The named constants only spare
//! callers (mostly tests) a string.

use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

/// A CLI Brigadier can drive.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, TS)]
#[ts(type = "string")]
pub struct ProviderKind(&'static str);

#[allow(non_upper_case_globals)]
impl ProviderKind {
    pub const Claude: Self = Self("claude");
    pub const Codex: Self = Self("codex");
    pub const OpenCode: Self = Self("opencode");

    /// An adapter's own id: lowercase letters, digits and `-`.
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// The id as written on the wire and in the CLI's flags.
    pub fn id(self) -> &'static str {
        self.0
    }

    /// The kind `id` names, if it is well formed. Unknown but well-formed ids are kept (a record
    /// from a CLI no longer registered still loads); what they name is the registry's question.
    pub fn parse(id: &str) -> Option<Self> {
        if id.is_empty()
            || id.len() > 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return None;
        }
        for known in [Self::Claude, Self::Codex, Self::OpenCode] {
            if known.0 == id {
                return Some(known);
            }
        }
        static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(found) = seen.iter().find(|seen| **seen == id) {
            return Some(Self(found));
        }
        // shortcut: ids are interned for good; bounded so a corrupt record can't grow it.
        if seen.len() >= 64 {
            return None;
        }
        let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
        seen.push(leaked);
        Some(Self(leaked))
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::fmt::Debug for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::str::FromStr for ProviderKind {
    type Err = String;

    fn from_str(id: &str) -> Result<Self, String> {
        Self::parse(id).ok_or_else(|| format!("not a CLI id: {id:?}"))
    }
}

impl Serialize for ProviderKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for ProviderKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = std::borrow::Cow::<str>::deserialize(deserializer)?;
        id.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderKind;

    #[test]
    fn ids_round_trip_and_unknown_ones_are_kept() {
        let json = serde_json::to_string(&ProviderKind::Codex).unwrap();
        assert_eq!(json, r#""codex""#);
        assert_eq!(
            serde_json::from_str::<ProviderKind>(&json).unwrap(),
            ProviderKind::Codex
        );
        let later: ProviderKind = serde_json::from_str(r#""newcli""#).unwrap();
        assert_eq!(later.id(), "newcli");
        assert_eq!(later, "newcli".parse().unwrap());
        assert!(serde_json::from_str::<ProviderKind>(r#""Bad Id""#).is_err());
        assert!(matches!(ProviderKind::Claude, ProviderKind::Claude));
    }
}
