//! Something the user adds: an MCP server or a skill, as read from what they pasted or picked,
//! before any agent has it. The Library packs it into a plugin every agent installs.

use std::path::PathBuf;

use crate::Secret;

/// An MCP server or a skill to add.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Server(ServerSpec),
    Skill(SkillSpec),
}

/// An MCP server to add.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSpec {
    /// The name its tools are called by.
    pub name: String,
    pub transport: TransportSpec,
}

/// How a server to add is reached.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportSpec {
    /// A process the CLI starts.
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, Secret)>,
        cwd: Option<PathBuf>,
    },
    /// Streamable HTTP (or SSE, which both CLIs read as a URL too).
    Http {
        url: String,
        headers: Vec<(String, Secret)>,
    },
}

/// A skill to add.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillSpec {
    /// Its name, when known before it is fetched (else its `SKILL.md`'s).
    pub name: Option<String>,
    pub source: SkillSource,
}

/// Where a skill to add comes from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillSource {
    /// A folder holding `SKILL.md` (or folders that each do).
    Folder(PathBuf),
    /// A Git repository, at a revision and in a folder of it when given.
    Git {
        url: String,
        rev: Option<String>,
        subdir: Option<String>,
    },
    /// A skills.sh id: `<owner>/<repo>/<skill>` or `<owner>/<repo>`.
    SkillsSh(String),
}
