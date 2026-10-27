//! The retired sharing format, read only by the one-time cleanup.
use super::PluginKind;
use brigadier_registry::Agent;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Sharing {
    pub version: u32,
    pub(super) chosen: BTreeMap<String, Choice>,
    pub(super) owned: Vec<Flag>,
    pub(super) pending: Option<Pending>,
    /// Each Library plugin sharing made, by its name.
    pub items: BTreeMap<String, Record>,
    /// Items the user removed: never shared again until Share with every agent.
    pub declined: Vec<Declined>,
    /// Library plugins whose removal started: finished by the next pass.
    pub pending_removal: Vec<String>,
    /// The projects a switched-off item was on in (`None`: every project), by page key: what
    /// switching it on again restores.
    pub off_scopes: BTreeMap<String, Option<Vec<PathBuf>>>,
    /// The last share that failed, by page key.
    pub failed: BTreeMap<String, Failure>,
    /// Other apps' servers brought in (`other:<id>`): no longer offered for import.
    pub imported: Vec<String>,
}

/// A Library plugin made from an agent's own item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    pub kind: PluginKind,
    /// The item's name.
    pub name: String,
    /// The original's fingerprint when it was packed (values hashed, never kept).
    pub fingerprint: String,
    /// The original's config file, or skill folder.
    pub file: PathBuf,
    /// The packed version.
    pub version: String,
    /// The version each agent took, by agent.
    #[serde(default)]
    pub applied: BTreeMap<String, String>,
    /// Passes running that read the original's file cleanly and found it gone.
    #[serde(default)]
    pub absent_passes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Declined {
    pub kind: PluginKind,
    pub name: String,
    /// A server's endpoint: the same server renamed stays declined.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// A skill's folder: the same skill renamed in it stays declined.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Failure {
    pub reason: String,
    /// What it was tried with: a pass while running tries again only once that changes.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Choice {
    pub on: bool,
    pub scope: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Flag {
    pub owner: String,
    pub file: PathBuf,
    /// Claude's key; None for Codex's user config.
    pub folder: Option<PathBuf>,
    pub name: String,
    pub claude: bool,
    pub copy: bool,
    pub before: Option<bool>,
    pub after: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pending {
    key: String,
    choice: Option<Choice>,
    flags: Vec<Flag>,
    id: Option<String>,
    before: Vec<(Agent, Placed)>,
    wants: Vec<(Agent, Placed)>,
    #[serde(default)]
    copies_done: bool,
    #[serde(default)]
    observed: Vec<(Agent, Placed)>,
    #[serde(default)]
    inflight: Option<Agent>,
    #[serde(default)]
    keep_safety: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Placed {
    /// It has it at all: installed, for every project or one, or on somewhere.
    pub has: bool,
    /// On in the user's scope: every project, but where a project turns it off.
    pub global: bool,
    /// The user's projects it is on in.
    pub on_in: Vec<PathBuf>,
}
