//! Listener and connector over the platform's local socket: a Unix domain socket inside a
//! private (0700) directory, or a named pipe whose DACL admits only the current user.

use std::io;
use std::time::Duration;

use brigadier_sandbox::{AppPaths, IpcEndpoint, Platform};
use interprocess::local_socket::tokio::{Listener as TokioListener, RecvHalf, SendHalf, Stream};
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{ListenerOptions, Name};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::AsyncReadExt as _;

use crate::Error;
use crate::frame::{FrameReader, FrameWriter};
use crate::protocol::{ClientFrame, ClientInfo, DaemonInfo, PROTOCOL_VERSION, ServerFrame};
use crate::token::Token;

/// Time a new connection has to present a valid first frame.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(2);

/// Largest first frame: a hello or an MCP grant.
const MAX_FIRST_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// A connection's raw byte stream, after its first frame.
pub type RawStream = Stream;

/// Reading half of a connection.
pub type Reader = FrameReader<RecvHalf>;
/// Writing half of a connection.
pub type Writer = FrameWriter<SendHalf>;

/// An authenticated connection, split into halves.
pub struct Connection {
    pub reader: Reader,
    pub writer: Writer,
}

impl Connection {
    fn new(stream: Stream) -> Self {
        let (recv, send) = stream.split();
        Self {
            reader: FrameReader::new(recv),
            writer: FrameWriter::new(send),
        }
    }
}

/// The daemon's listening endpoint.
pub struct Listener {
    inner: TokioListener,
}

impl Listener {
    /// Binds the endpoint. The caller must hold the instance lock: a leftover Unix socket from
    /// a crashed daemon is removed.
    pub fn bind(platform: &dyn Platform) -> Result<Self, Error> {
        let paths = platform.paths();
        platform.private_fs().create_private_dir(&paths.run_dir)?;
        if let Some(dir) = paths.socket_dir() {
            platform.private_fs().create_private_dir(dir)?;
        }
        let options = ListenerOptions::new().name(endpoint_name(&paths.ipc_endpoint)?);

        #[cfg(unix)]
        let options = {
            if let IpcEndpoint::UnixSocket(path) = &paths.ipc_endpoint {
                remove_stale_socket(path)?;
            }
            options.reclaim_name(true)
        };
        #[cfg(windows)]
        let options = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            use interprocess::os::windows::security_descriptor::SecurityDescriptor;
            let sddl = brigadier_sandbox::windows::current_user_only_sddl(false)?;
            let sddl = widestring::U16CString::from_str(&sddl)
                .map_err(|err| Error::Io(io::Error::new(io::ErrorKind::InvalidInput, err)))?;
            options.security_descriptor(SecurityDescriptor::deserialize(&sddl)?)
        };

        Ok(Self {
            inner: options.create_tokio()?,
        })
    }

    /// Waits for the next raw connection. Authenticate it with [`Pending::authenticate`] on its
    /// own task so a slow client never holds up the accept loop.
    pub async fn accept(&self) -> Result<Pending, Error> {
        Ok(Pending {
            stream: self.inner.accept().await?,
        })
    }
}

/// A connection that has not yet sent its first frame.
pub struct Pending {
    stream: Stream,
}

/// What a new connection turned out to be, from its first frame.
pub enum Accepted {
    /// A token holder (the app): a framed request/event connection.
    Client {
        connection: Connection,
        client: ClientInfo,
    },
    /// A CLI session's Brigadier MCP bridge (`brigadierd mcp`). The stream carries raw MCP from
    /// here on, starting with the first byte after the frame.
    Mcp { grant: String, stream: RawStream },
}

impl Pending {
    /// Reads the first frame within [`AUTH_TIMEOUT`]. A hello must carry the token and the
    /// protocol version; an MCP frame carries a grant the caller checks. Any failure
    /// drops the connection without a reply.
    ///
    /// The first frame is read unbuffered, so nothing the peer sent after it is lost when the
    /// connection switches to raw MCP.
    pub async fn handshake(self, token: &Token) -> Result<Accepted, Error> {
        let mut stream = self.stream;
        let first = tokio::time::timeout(AUTH_TIMEOUT, read_first_frame(&mut stream))
            .await
            .map_err(|_| Error::Unauthorized("no first frame before the deadline"))?;
        match first {
            Ok(Some(ClientFrame::Hello {
                token: presented,
                protocol,
                client,
            })) => {
                if !token.matches(&presented) {
                    return Err(Error::Unauthorized("wrong token"));
                }
                if protocol != PROTOCOL_VERSION {
                    return Err(Error::Unauthorized("unsupported protocol version"));
                }
                if peer_confined(&stream) {
                    return Err(Error::Unauthorized(
                        "a sandboxed process presented the token",
                    ));
                }
                Ok(Accepted::Client {
                    connection: Connection::new(stream),
                    client,
                })
            }
            Ok(Some(ClientFrame::Mcp { grant })) => Ok(Accepted::Mcp { grant, stream }),
            Ok(Some(ClientFrame::Request { .. })) => {
                Err(Error::Unauthorized("first frame was not a hello"))
            }
            Ok(None) => Err(Error::Unauthorized("closed before the first frame")),
            // Oversized or malformed first frames are rejected here too.
            Err(_) => Err(Error::Unauthorized("malformed first frame")),
        }
    }
}

/// Whether the peer runs in an OS sandbox this daemon is not in (see
/// [`brigadier_sandbox::peer_confined`]): the token alone never lets a confined CLI session
/// act as the app.
#[cfg(unix)]
fn peer_confined(stream: &Stream) -> bool {
    use std::os::fd::AsFd as _;
    match stream {
        Stream::UdSocket(socket) => brigadier_sandbox::peer_confined(socket.as_fd()),
    }
}

/// Named pipes admit only the current user; Windows sandboxing is not built yet.
#[cfg(windows)]
fn peer_confined(_stream: &Stream) -> bool {
    false
}

/// Reads one length-prefixed frame straight from the stream, without read-ahead.
async fn read_first_frame(stream: &mut Stream) -> Result<Option<ClientFrame>, Error> {
    let mut len = [0u8; 4];
    match stream.read_exact(&mut len).await {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FIRST_FRAME_BYTES {
        return Err(Error::FrameTooLarge(len));
    }
    let mut buffer = vec![0u8; len];
    stream.read_exact(&mut buffer).await?;
    Ok(Some(serde_json::from_slice(&buffer)?))
}

fn encode_frame<T: Serialize>(frame: &T) -> Result<Vec<u8>, Error> {
    let json = serde_json::to_vec(frame)?;
    if json.len() > crate::MAX_FRAME_BYTES {
        return Err(Error::FrameTooLarge(json.len()));
    }
    let mut bytes = Vec::with_capacity(json.len() + 4);
    bytes.extend_from_slice(&(json.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

/// Connects to the daemon for `paths` without an async runtime and sends `first`, a
/// [`ClientFrame::Mcp`]. For the short-lived helper process CLI sessions start (`brigadierd
/// mcp`), which never reads the token.
pub fn connect_blocking(
    paths: &AppPaths,
    first: &ClientFrame,
) -> Result<interprocess::local_socket::Stream, Error> {
    use interprocess::local_socket::traits::Stream as _;
    use std::io::Write as _;
    let mut stream =
        interprocess::local_socket::Stream::connect(endpoint_name(&paths.ipc_endpoint)?)?;
    stream.write_all(&encode_frame(first)?)?;
    stream.flush()?;
    Ok(stream)
}

/// Reads one length-prefixed frame from a blocking reader; `None` when the peer closed the
/// connection before it.
pub fn read_frame_blocking<T: DeserializeOwned>(
    reader: &mut impl io::Read,
) -> Result<Option<T>, Error> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > crate::MAX_FRAME_BYTES {
        return Err(Error::FrameTooLarge(len));
    }
    let mut buffer = vec![0u8; len];
    reader.read_exact(&mut buffer)?;
    Ok(Some(serde_json::from_slice(&buffer)?))
}

/// Connects to a running daemon and completes the handshake.
pub async fn connect(
    platform: &dyn Platform,
    client: ClientInfo,
) -> Result<(Connection, DaemonInfo, i64), Error> {
    connect_to(platform.paths(), client).await
}

/// Connects to the daemon of the data directory `paths` describes (not necessarily this
/// process's own) and completes the handshake with its token.
pub async fn connect_to(
    paths: &AppPaths,
    client: ClientInfo,
) -> Result<(Connection, DaemonInfo, i64), Error> {
    let token = Token::read(&paths.token_path)?;
    let stream = Stream::connect(endpoint_name(&paths.ipc_endpoint)?).await?;
    let mut connection = Connection::new(stream);
    connection
        .writer
        .write(&ClientFrame::Hello {
            token: token.as_str().to_owned(),
            protocol: PROTOCOL_VERSION,
            client,
        })
        .await?;
    let welcome = tokio::time::timeout(AUTH_TIMEOUT, connection.reader.read::<ServerFrame>())
        .await
        .map_err(|_| Error::Unauthorized("daemon did not answer the hello"))??;
    match welcome {
        Some(ServerFrame::Welcome { daemon, last_seq }) => Ok((connection, daemon, last_seq)),
        Some(_) => Err(Error::Protocol("expected welcome")),
        None => Err(Error::Unauthorized("daemon rejected the hello")),
    }
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
            "IPC endpoint type not available on this platform",
        )),
    }
}

#[cfg(unix)]
fn remove_stale_socket(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists and is not a socket", path.display()),
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
