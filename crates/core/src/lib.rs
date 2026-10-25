//! Brigadier core: the domain model and the session manager.
//!
//! The event store is the source of truth. The core keeps a small in-memory projection of the
//! catalog (projects, sessions, chats, settings) rebuilt from the log on start, and records
//! every change as a [`DomainEvent`].

pub mod accounts;
pub mod app_call;
mod board;
pub mod browser;
pub mod computer;
pub mod digest;
mod documents;
pub mod feedback;
pub mod knowledge;
pub mod ledger;
mod logging;
mod machine;
pub mod manager;
pub mod model;
pub mod pages;
mod projection;
pub mod routing;
pub mod runtime;
mod sessions;
pub mod storage;
pub mod thread_view;
pub mod tools;
pub mod work;

pub use app_call::*;
pub use browser::*;
pub use computer::*;
pub use feedback::*;
pub use knowledge::*;
pub use model::*;
pub use sessions::{Core, MAX_ATTACHMENT_BYTES, ProbeBurst};
pub use work::*;

/// Errors surfaced to clients.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Store(#[from] brigadier_store::Error),
    /// A CLI adapter failed.
    #[error("{0}")]
    Provider(String),
    #[error("corrupt event {seq}: {source}")]
    Corrupt { seq: i64, source: serde_json::Error },
}

impl Error {
    /// The store stopped admitting writes because the daemon is quitting.
    pub fn is_shutting_down(&self) -> bool {
        matches!(self, Self::Store(brigadier_store::Error::ShuttingDown))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// `make`'s future, made inside this call and polled from the heap: the caller keeps a pointer
/// to it, not the future. For a dispatching `match` whose arms await engine futures of tens of
/// KB: an unoptimized build gives every `.await` of a function its own stack slots, all of them
/// at once, and the IPC dispatch had grown past 1.2 MB of a runtime thread's 2 MiB stack. A
/// `Box::pin` at the await shrinks the caller's future but still makes the future in the
/// caller's frame.
pub fn heap<F: std::future::Future>(make: impl FnOnce() -> F) -> std::pin::Pin<Box<F>> {
    Box::pin(make())
}

/// How long a test may wait for what it needs before it fails: only a hang takes this long. On
/// CI it stays under nextest's kill of a test hung for two minutes (`.config/nextest.toml`), so
/// a hang fails saying what it waited for instead of being killed (#111).
#[cfg(test)]
pub(crate) fn test_patience() -> std::time::Duration {
    let secs = if std::env::var_os("CI").is_some() {
        100
    } else {
        300
    };
    std::time::Duration::from_secs(secs)
}

/// A test's git command must use its scratch repository, even inside a hook or bisect run.
/// Its commits start no `git maintenance --detach`: that lives on in the repository after the
/// commit returns, so a test that then asks what runs in the folder would find it (#586).
#[cfg(test)]
pub(crate) fn test_git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.args(["-c", "maintenance.auto=false"]);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_PREFIX",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    command
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// This daemon process's own id, new each start: what it recorded about live CLIs (their MCP
/// servers) says nothing once it restarted.
pub fn boot_id() -> &'static str {
    static BOOT: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());
    &BOOT
}
