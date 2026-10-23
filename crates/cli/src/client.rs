//! Reaches the daemon over its local socket (a Unix socket, or a named pipe on Windows) and
//! makes one call.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use brigadier_sandbox::{AppPaths, IpcEndpoint};
use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{Name, Stream};
use serde::Serialize;

use crate::protocol::{CliCall, CliReply, ErrorCode, GRANT_ENV, Heard, PROTOCOL, SESSION_ENV};

/// The largest reply read (the daemon's frames are capped well under it).
const MAX_REPLY_BYTES: usize = 64 * 1024 * 1024;

/// The connection's first frame, as the daemon's IPC protocol has it (`ClientFrame::Cli`).
#[derive(Serialize)]
#[serde(tag = "type", rename = "cli")]
pub struct FirstFrame<'a> {
    pub session: &'a str,
    pub grant: &'a str,
}

/// Who calls: the session's id and grant, from its environment.
pub struct Caller {
    pub session: String,
    pub grant: String,
}

impl Caller {
    /// From [`SESSION_ENV`] and [`GRANT_ENV`]; a refusal when either is missing.
    pub fn from_env() -> Result<Self, CliReply> {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        match (read(SESSION_ENV), read(GRANT_ENV)) {
            (Some(session), Some(grant)) => Ok(Self { session, grant }),
            _ => Err(CliReply::error(
                ErrorCode::Refused,
                format!(
                    "refused: this shell is not a Brigadier session ({SESSION_ENV} and \
                     {GRANT_ENV} are not set)"
                ),
            )),
        }
    }
}

/// The daemon's socket, from `BRIGADIER_DATA_DIR` or the installed app's data folder.
pub fn daemon_paths() -> Result<AppPaths, CliReply> {
    let unreachable = |err: &dyn std::fmt::Display| {
        CliReply::error(
            ErrorCode::Unreachable,
            format!("cannot find Brigadier: {err}"),
        )
    };
    let dir = match std::env::var_os(brigadier_sandbox::DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => brigadier_sandbox::default_data_dir().map_err(|err| unreachable(&err))?,
    };
    AppPaths::resolve(dir).map_err(|err| unreachable(&err))
}

/// An answer, with the connection it came on while it is open.
pub struct Answered {
    pub reply: CliReply,
    stream: Option<Stream>,
}

impl Answered {
    fn closed(reply: CliReply) -> Self {
        Self {
            reply,
            stream: None,
        }
    }

    /// Tells Brigadier the events of a `wait`'s answer were printed: they were heard. Without
    /// it, they are its workers' events again for the next `wait`.
    pub fn heard(self) {
        if let Some(mut stream) = self.stream {
            let _ = write_frame(&mut stream, &Heard { heard: true });
        }
    }
}

/// Makes `call` for `caller`: one frame each way (and [`Answered::heard`] after a `wait`).
pub fn call(paths: &AppPaths, caller: &Caller, call: &CliCall) -> Answered {
    let unreachable = |err: io::Error| {
        Answered::closed(CliReply::error(
            ErrorCode::Unreachable,
            format!("cannot reach Brigadier (is the app running?): {err}"),
        ))
    };
    let mut stream = match endpoint_name(&paths.ipc_endpoint).and_then(Stream::connect) {
        Ok(stream) => stream,
        Err(err) => return unreachable(err),
    };
    let first = FirstFrame {
        session: &caller.session,
        grant: &caller.grant,
    };
    let sent = write_frame(&mut stream, &first).and_then(|()| write_frame(&mut stream, call));
    if let Err(err) = sent {
        // A refused grant closes the connection before the request is read.
        if matches!(
            err.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
        ) {
            return Answered::closed(refused());
        }
        return unreachable(err);
    }
    match read_frame(&mut stream) {
        Ok(Some(reply)) => Answered {
            reply,
            stream: Some(stream),
        },
        // Closed without a word: the grant was refused.
        Ok(None) => Answered::closed(refused()),
        Err(err) if err.kind() == io::ErrorKind::ConnectionReset => Answered::closed(refused()),
        Err(err) => unreachable(err),
    }
}

fn refused() -> CliReply {
    CliReply::error(
        ErrorCode::Refused,
        format!(
            "refused: Brigadier doesn't know this session, or it ended ({SESSION_ENV} / \
             {GRANT_ENV})"
        ),
    )
}

/// The protocol version this binary speaks, wrapped around `request`.
pub fn wrap(request: crate::protocol::CliRequest) -> CliCall {
    CliCall {
        protocol: PROTOCOL,
        request,
    }
}

fn write_frame(stream: &mut Stream, value: &impl Serialize) -> io::Result<()> {
    let json = serde_json::to_vec(value).map_err(io::Error::other)?;
    let len = u32::try_from(json.len()).map_err(io::Error::other)?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&json)?;
    stream.flush()
}

fn read_frame(stream: &mut Stream) -> io::Result<Option<CliReply>> {
    let mut len = [0u8; 4];
    match stream.read_exact(&mut len) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_REPLY_BYTES {
        return Err(io::Error::other(format!("a {len}-byte reply is too large")));
    }
    let mut buffer = vec![0u8; len];
    stream.read_exact(&mut buffer)?;
    serde_json::from_slice(&buffer)
        .map(Some)
        .map_err(io::Error::other)
}

fn endpoint_name(endpoint: &IpcEndpoint) -> io::Result<Name<'static>> {
    match endpoint {
        #[cfg(unix)]
        IpcEndpoint::UnixSocket(path) => {
            use interprocess::local_socket::{GenericFilePath, ToFsName};
            path.clone().to_fs_name::<GenericFilePath>()
        }
        #[cfg(windows)]
        IpcEndpoint::NamedPipe(name) => {
            use interprocess::local_socket::{GenericNamespaced, ToNsName};
            name.clone().to_ns_name::<GenericNamespaced>()
        }
        #[allow(unreachable_patterns)]
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this kind of socket is not available on this platform",
        )),
    }
}
