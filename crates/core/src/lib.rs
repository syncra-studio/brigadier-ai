//! Brigadier core: the domain model and the session manager.
//!
//! The event store is the source of truth. The core keeps a small in-memory projection of the
//! catalog (projects, sessions, chats, settings) rebuilt from the log on start, and records
//! every change as a [`DomainEvent`].

pub mod accounts;
mod board;
pub mod digest;
pub mod knowledge;
pub mod ledger;
mod machine;
pub mod manager;
pub mod model;
pub mod overnight;
pub mod pages;
mod projection;
pub mod routing;
pub mod runtime;
mod sessions;
pub mod storage;
pub mod tools;
pub mod work;

pub use knowledge::*;
pub use model::*;
pub use overnight::*;
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

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
