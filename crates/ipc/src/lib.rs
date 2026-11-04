//! Authenticated local IPC between the Brigadier app and `brigadierd`.
//!
//! - Transport: a Unix domain socket in a private directory, or a named pipe restricted to the
//!   current user on Windows. There is no TCP listener.
//! - Authentication: a per-launch 256-bit token in a private file; the first frame must present
//!   it within [`transport::AUTH_TIMEOUT`]. CLI sessions' helper process (the MCP bridge) never
//!   sees the token: its first frame carries a role-scoped grant instead, which only reaches
//!   the MCP tools (or, for a `brigadier` command, its read commands).
//! - Framing: length-prefixed JSON, at most [`MAX_FRAME_BYTES`] per frame.
//! - Types: [`protocol`] and [`metrics`], exported to TypeScript by the `gen-ts` binary.

pub mod app;
pub mod frame;
pub mod metrics;
pub mod protocol;
pub mod token;
pub mod transport;

pub use token::Token;
pub use transport::{
    Accepted, Connection, Hello, Listener, Pending, RawStream, Reader, Writer, connect,
    connect_blocking, connect_to, encode_frame, hello, read_frame_blocking,
};

/// Largest frame accepted in either direction.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame of {0} bytes exceeds the limit")]
    FrameTooLarge(usize),
    #[error("unauthorized: {0}")]
    Unauthorized(&'static str),
    #[error("protocol: {0}")]
    Protocol(&'static str),
    /// The daemon closed the connection without answering the hello: a wrong token, or a
    /// daemon from before [`protocol::ServerFrame::ProtocolMismatch`] speaking another
    /// protocol version.
    #[error("the daemon closed the connection without answering the hello")]
    Rejected,
    /// The daemon speaks another protocol version (`protocol` of its [`protocol::DaemonInfo`]).
    #[error("the daemon (version {}) speaks protocol {}, this build {}", .0.version, .0.protocol, protocol::PROTOCOL_VERSION)]
    ProtocolMismatch(Box<protocol::DaemonInfo>),
    #[error(transparent)]
    Platform(#[from] brigadier_sandbox::Error),
}
