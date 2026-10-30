//! The CLIs Brigadier drives: each adapter's static facts and how to make it. Adding a CLI is
//! one adapter module and one line in [`ADAPTERS`]; nothing else names a CLI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_sandbox::Platform;
use serde_json::Value;

use crate::cli::CliEnv;
use crate::leftovers::FoundSession;
use crate::login_mark::LoginMark;
use crate::session_file::{FileContext, FileEntry};
use crate::{Provider, ProviderKind};

/// A session file kept read, to read again for only what was added since
/// ([`crate::session_file::Reading`]).
pub(crate) type Follow = Box<dyn FnMut() -> std::io::Result<Vec<FileEntry>> + Send>;

/// Makes an adapter, for the platform and the CLIs' environment.
pub type Make = fn(Arc<dyn Platform>, Arc<CliEnv>) -> Arc<dyn Provider>;

/// Finds a CLI's sessions ([`crate::leftovers::find`]).
type Leftovers = fn(&Path, &[PathBuf], &dyn Fn(&Path) -> bool) -> Vec<FoundSession>;

/// One CLI's adapter: what Brigadier knows of it before running it.
pub struct Adapter {
    pub kind: ProviderKind,
    /// The product name shown to the user ("Claude Code").
    pub label: &'static str,
    /// The binary name looked up on the login shell's PATH.
    pub binary: &'static str,
    /// The vendor's own installer, as typed into a terminal: `[unix, windows]`.
    pub install: [&'static str; 2],
    /// Signs the user in to the CLI, as typed into a terminal.
    pub login: &'static str,
    /// The CLI's own sign-in arguments (`auth login`), for an account's terminal.
    pub sign_in: &'static [&'static str],
    /// The CLI's own sign-out arguments (`auth logout`).
    pub sign_out: &'static [&'static str],
    /// Its account homes, for a CLI that takes more than one login.
    pub home: Option<&'static crate::accounts::Home>,
    /// What starts a skill in a message (`/` or `$`).
    pub skill_prefix: &'static str,
    /// The built-in tool capabilities its adapter implements, by their model registry names
    /// (`webSearch`, `imageGeneration`): a registry can't enable others.
    pub tools: &'static [&'static str],
    /// The kinds of its [`crate::Artifact::CliFile`]s that go with a session's transcript, so
    /// they are kept with it (under its conversation's lineage).
    pub session_files: &'static [&'static str],
    /// Its short vendor word, in routing and report titles and error lines ("Claude").
    pub vendor: &'static str,
    /// Who makes it, on its Settings page's line.
    pub maker: &'static str,
    /// Its CLI as the Settings rows name it.
    pub cli_name: &'static str,
    /// The vendor's setup docs.
    pub install_guide: &'static str,
    /// The usage windows its quota usually reports, the first its shortest (the fault
    /// injector's default).
    pub usual_windows: &'static [&'static str],
    /// Its mark.
    pub glyph: Glyph,
    /// It runs only the first skill in a message: one skill chip per message.
    pub one_skill_per_message: bool,
    /// Its usage reports cache writes; one that doesn't is judged by its cached share of input.
    pub reports_cache_writes: bool,
    /// Its built-in commands and skills run when sent as message text (`/compact`, `/<skill>`),
    /// so its transcript keeps them as the user's words; otherwise Brigadier runs them as
    /// requests of their own.
    pub commands_as_text: bool,
    /// The tool names its native subagents start with.
    pub subagent_tools: &'static [&'static str],
    /// Its built-in `/` commands Brigadier offers: name, description, and what runs it (the
    /// name sent as text, or its protocol's request).
    pub builtin_commands: &'static [(&'static str, &'static str, &'static str)],
    /// Its `/` menu lists the MCP servers' prompts (`/mcp__<server>__<prompt>`).
    pub mcp_prompts: bool,
    /// Its usage reports are totals over the session's whole life: a resumed session's first
    /// report only sets the baseline.
    pub cumulative_usage: bool,
    /// The model family a one-off chore runs on first (its cheapest).
    pub chore_family: &'static str,
    /// The effort a one-off chore runs at, where it takes one.
    pub chore_effort: Option<&'static str>,
    /// Only its instructions keep it from adding AI co-author lines (another CLI's settings
    /// hold it to that).
    pub coauthors_in_instructions: bool,
    /// How it compacts in place for a long session, at Brigadier's point in tokens (and
    /// whether the user set that point); none: it isn't run as one.
    pub compaction: Option<fn(i64, bool) -> crate::NativeCompaction>,
    /// Brigadier asks it to compact at its point, with what the summary must keep; otherwise
    /// it compacts by itself at the limit it was given.
    pub brigadier_compacts: bool,
    /// Its subagents' transcripts are files of their own beside the session's
    /// (`<session>/subagents/`).
    pub subagent_transcripts: bool,
    /// A session gets Brigadier's output hook, which trims long command output.
    pub output_hook: bool,
    /// Pausing lets the command it runs finish first.
    pub pause_finishes_command: bool,
    /// Its auto-review runs in child sessions whose use the parent's totals leave out.
    pub child_sessions: bool,
    /// Its sessions are removed through the CLI itself, by id, working in its own home.
    pub removes_by_id: bool,
    /// Its sign-in prints a page to paste a code back from.
    pub sign_in_code_page: bool,
    /// Its Windows installer leaves its folder off the user's PATH.
    pub installer_off_path: bool,
    /// The [`crate::Artifact::CliFile`] kinds whose path is disk space of its own.
    pub space_files: &'static [&'static str],
    /// The [`crate::Artifact::CliFile`] kind of a session's own temp folder
    /// (`/tmp/brigadier-<tag>-…`), when it makes one.
    pub temp_dir: Option<&'static str>,
    /// Its npm package, which its updater follows.
    pub npm_package: &'static str,
    /// What its settings allow its updater to install.
    pub update_limits: fn(&CliEnv) -> UpdateLimits,
    /// Gives a session the skills Brigadier staged in an overlay folder
    /// (`<overlay>/.claude/skills/<name>`).
    pub skills_overlay: fn(&mut crate::SessionSpec, PathBuf) -> std::io::Result<()>,
    /// Makes the adapter.
    pub new: Make,
    /// Whether the first line of a binary's `--version` names this CLI ([`crate::cli::check_binary`]).
    pub(crate) names_itself: fn(&str) -> bool,
    /// Where its native installer puts the binary ([`CliEnv::installer_folder`]).
    pub(crate) installer_folder: fn(&CliEnv) -> Option<PathBuf>,
    /// Where the agent run with an environment keeps its login ([`LoginMark::of`]).
    pub(crate) login_mark: fn(&CliEnv) -> Option<LoginMark>,
    /// The stores of MCP sign-ins of the account at a home, for that account's environment
    /// ([`crate::mcp_oauth`]).
    pub(crate) mcp_stores: fn(&CliEnv, &Path) -> Vec<crate::mcp_oauth::Store>,
    /// Its session file read back as a thread ([`crate::session_file::read`]).
    pub(crate) read_session: fn(&Path) -> std::io::Result<Vec<FileEntry>>,
    /// The context size its session file last recorded ([`crate::session_file::context`]).
    pub(crate) session_context: fn(&Path) -> std::io::Result<Option<FileContext>>,
    /// Its session file kept read ([`crate::session_file::Reading`]).
    pub(crate) follow_session: fn(&Path) -> Follow,
    /// Where a session file recorded at a path that is gone may be now
    /// ([`crate::session_file::locate`]).
    pub(crate) moved_session_file: fn(&Path) -> Option<PathBuf>,
    /// Its sessions found in its homes ([`crate::leftovers::find`]): the history home, every
    /// home, and the working folders wanted.
    pub(crate) leftovers: Leftovers,
    /// For a streamed delta line of its stream: its stream key, its item and where its text is
    /// ([`crate::record`]).
    pub(crate) delta_location: fn(&Value) -> Option<(String, String, &'static str)>,
    /// The delta streams a line of its stream ends: `Some(item)` for one item's, `None` for all.
    pub(crate) stream_end: fn(&Value) -> Option<Option<String>>,
    /// A turn that ends because the usage window ran out, in its wire format, at a time in
    /// seconds since the epoch ([`crate::simulate::usage_limit`]).
    pub(crate) usage_limit: fn(i64) -> Vec<Value>,
    /// How it keeps folder trust in its own settings, when it does ([`crate::trust`]).
    pub(crate) trust: Option<&'static crate::trust::Trust>,
}

/// A CLI's mark as one SVG path.
pub struct Glyph {
    pub view_box: &'static str,
    pub path: &'static str,
    /// Its brand colour's class; none: the current colour.
    pub class_name: Option<&'static str>,
}

/// What a CLI's settings allow its updater to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateLimits {
    /// The npm dist-tag its updater follows.
    pub channel: &'static str,
    /// The newest version it may run.
    pub maximum: Option<String>,
    /// Updates are turned off altogether.
    pub updates_off: bool,
}

impl Default for UpdateLimits {
    fn default() -> Self {
        Self {
            channel: "latest",
            maximum: None,
            updates_off: false,
        }
    }
}

/// Every registered CLI, in the order the app lists them.
pub static ADAPTERS: &[&Adapter] = &[
    &crate::claude::ADAPTER,
    &crate::codex::ADAPTER,
    &crate::opencode::ADAPTER,
];

/// `kind`'s adapter, when it is registered.
pub fn adapter(kind: ProviderKind) -> Option<&'static Adapter> {
    ADAPTERS
        .iter()
        .copied()
        .find(|adapter| adapter.kind == kind)
}

/// Every registered CLI's kind, in [`ADAPTERS`] order.
pub fn kinds() -> impl Iterator<Item = ProviderKind> {
    ADAPTERS.iter().map(|adapter| adapter.kind)
}

/// The CLI a conversation without a choice of its own runs on: the first registered.
pub fn default_kind() -> ProviderKind {
    ADAPTERS[0].kind
}
