//! The model advice view; none of its guidance is an enforcement list.
use crate::protocol::Agent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsOutput {
    pub models: Vec<Model>,
    /// Each agent's account switch (Settings › Providers › the agent's page).
    pub agents: Vec<AgentAccounts>,
    /// "Switch to another agent automatically": a session whose agent has every account at its
    /// limit goes on with another agent.
    pub switch_providers: bool,
    pub accounts: Vec<Account>,
    pub guidance: Vec<Guidance>,
    pub rules: Vec<Rule>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub agent: Agent,
    pub model: String,
    pub available: bool,
    pub efforts: Vec<String>,
    pub age_s: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAccounts {
    pub agent: Agent,
    /// On: Brigadier orders the accounts (`place`, `why`); off: the account picked with Use first.
    pub switch_accounts: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub agent: Agent,
    pub current: bool,
    pub you: bool,
    pub logged_in: bool,
    pub age_s: Option<i64>,
    pub fit: String,
    pub usage: Vec<Usage>,
    /// Its place in the order work goes to its agent's accounts (1 first); null while it can't
    /// take work.
    pub place: Option<u32>,
    /// Why it stands there, in words.
    pub why: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub window: String,
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub model: Option<String>,
    pub bucket: Option<String>,
    pub pooled: bool,
    pub runway_ms: Option<i64>,
    pub estimate_rough: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Guidance {
    pub work: String,
    pub agent: Agent,
    pub model: String,
    pub effort: Option<String>,
    pub reason: String,
    pub source: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub rule: String,
}
