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

/// Largest first frame: a hello, or a hook's or a command's grant.
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
    /// A Claude thread's output hook (`brigadierd hook post-tool-use`): `output.bytes` raw
    /// bytes follow on the stream, and the daemon answers with one length-prefixed
    /// [`crate::protocol::HookReply`] ([`encode_frame`]).
    Hook {
        grant: String,
        output: crate::protocol::HookOutput,
        stream: RawStream,
    },
    /// A session's `brigadier` command: one request frame follows on the connection, and the
    /// daemon answers with one frame (`brigadier_cli::protocol`).
    Cli {
        session: String,
        grant: String,
        connection: Connection,
    },
    /// A token holder speaking another protocol version (`protocol`): an app of another
    /// version. The daemon answers with [`ServerFrame::ProtocolMismatch`] and takes nothing
    /// but a shutdown on it, so the app is not left waiting on a daemon it can't reach.
    Mismatched {
        connection: Connection,
        client: ClientInfo,
        protocol: u32,
    },
}

impl Pending {
    /// Reads the first frame within [`AUTH_TIMEOUT`]. A hello must carry the token; one of
    /// another protocol version is [`Accepted::Mismatched`]. A hook's or a command's frame
    /// carries a grant the caller checks. Any failure drops the connection without a reply.
    ///
    /// The first frame is read unbuffered, so nothing the peer sent after it is lost when a
    /// hook's raw output follows it.
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
                if peer_confined(&stream) {
                    return Err(Error::Unauthorized(
                        "a sandboxed process presented the token",
                    ));
                }
                if protocol != PROTOCOL_VERSION {
                    return Ok(Accepted::Mismatched {
                        connection: Connection::new(stream),
                        client,
                        protocol,
                    });
                }
                Ok(Accepted::Client {
                    connection: Connection::new(stream),
                    client,
                })
            }
            Ok(Some(ClientFrame::Hook { grant, output })) => Ok(Accepted::Hook {
                grant,
                output,
                stream,
            }),
            Ok(Some(ClientFrame::Cli { session, grant })) => Ok(Accepted::Cli {
                session,
                grant,
                connection: Connection::new(stream),
            }),
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

/// One length-prefixed JSON frame.
pub fn encode_frame<T: Serialize>(frame: &T) -> Result<Vec<u8>, Error> {
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
/// [`ClientFrame::Hook`]. For the short-lived hook processes CLI sessions start (`brigadierd
/// hook`), which never read the token.
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
    match hello(paths, client).await? {
        Hello::Welcome {
            connection,
            daemon,
            last_seq,
        } => Ok((connection, daemon, last_seq)),
        Hello::Mismatch { daemon, .. } => Err(Error::ProtocolMismatch(Box::new(daemon))),
    }
}

/// How a daemon answered a hello.
pub enum Hello {
    /// It took this client: the connection is ready for requests.
    Welcome {
        connection: Connection,
        daemon: DaemonInfo,
        last_seq: i64,
    },
    /// It speaks another protocol version; the connection takes only a shutdown request.
    Mismatch {
        connection: Connection,
        daemon: DaemonInfo,
    },
}

/// Connects to the daemon of the data directory `paths` describes and says hello with its
/// token. A daemon that closes without answering is [`Error::Rejected`].
pub async fn hello(paths: &AppPaths, client: ClientInfo) -> Result<Hello, Error> {
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
        Some(ServerFrame::Welcome { daemon, last_seq }) => Ok(Hello::Welcome {
            connection,
            daemon,
            last_seq,
        }),
        Some(ServerFrame::ProtocolMismatch { daemon }) => {
            Ok(Hello::Mismatch { connection, daemon })
        }
        Some(_) => Err(Error::Protocol("expected welcome")),
        None => Err(Error::Rejected),
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

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_sandbox::PlatformOptions;

    /// A data directory of its own, with a published token and a bound endpoint.
    struct Daemon {
        dir: std::path::PathBuf,
        platform: std::sync::Arc<dyn Platform>,
        token: Token,
        listener: Listener,
    }

    impl Daemon {
        fn start(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("win-issues-w10-ipc-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let platform = brigadier_sandbox::native(PlatformOptions {
                data_dir: Some(dir.clone()),
            })
            .unwrap();
            platform
                .private_fs()
                .create_private_dir(&platform.paths().run_dir)
                .unwrap();
            let token = Token::generate().unwrap();
            token
                .publish(&*platform, &platform.paths().token_path)
                .unwrap();
            let listener = Listener::bind(&*platform).unwrap();
            Self {
                dir,
                platform,
                token,
                listener,
            }
        }

        fn info(&self, protocol: u32) -> DaemonInfo {
            DaemonInfo {
                version: "0.0.1-old".into(),
                protocol,
                pid: 7,
                platform: "test".into(),
                started_at_ms: 0,
                data_dir: self.dir.display().to_string(),
            }
        }
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
            if let Some(dir) = self.platform.paths().socket_dir() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    fn client() -> ClientInfo {
        ClientInfo {
            name: "test".into(),
            pid: std::process::id(),
        }
    }

    /// Says hello as an app of protocol `protocol` with `token`, and reads the answer.
    async fn hello_as(paths: &AppPaths, token: &str, protocol: u32) -> Option<ServerFrame> {
        let (paths, token) = (paths.clone(), token.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut stream = connect_blocking(
                &paths,
                &ClientFrame::Hello {
                    token,
                    protocol,
                    client: client(),
                },
            )
            .unwrap();
            read_frame_blocking::<ServerFrame>(&mut stream).unwrap_or(None)
        })
        .await
        .unwrap()
    }

    /// An app of another version that holds the token is told who the daemon is, not dropped
    /// unanswered (#771); one without the token still gets nothing.
    #[tokio::test]
    async fn a_hello_of_another_protocol_with_the_token_is_answered() {
        let daemon = Daemon::start("mismatch");
        let paths = daemon.platform.paths().clone();
        let newer = PROTOCOL_VERSION + 1;
        let asking = tokio::spawn({
            let (paths, token) = (paths.clone(), daemon.token.as_str().to_owned());
            async move { hello_as(&paths, &token, newer).await }
        });
        let accepted = daemon
            .listener
            .accept()
            .await
            .unwrap()
            .handshake(&daemon.token)
            .await;
        let Ok(Accepted::Mismatched {
            mut connection,
            protocol,
            ..
        }) = accepted
        else {
            panic!("a token holder of protocol {newer} was not told apart");
        };
        assert_eq!(protocol, newer);
        let info = daemon.info(PROTOCOL_VERSION);
        connection
            .writer
            .write(&ServerFrame::ProtocolMismatch {
                daemon: info.clone(),
            })
            .await
            .unwrap();
        let Some(ServerFrame::ProtocolMismatch { daemon: told }) = asking.await.unwrap() else {
            panic!("no answer");
        };
        assert_eq!(told, info);

        let asking = tokio::spawn({
            let paths = paths.clone();
            async move { hello_as(&paths, "not the token", newer).await }
        });
        let accepted = daemon
            .listener
            .accept()
            .await
            .unwrap()
            .handshake(&daemon.token)
            .await;
        assert!(matches!(accepted, Err(Error::Unauthorized("wrong token"))));
        drop(accepted);
        assert!(asking.await.unwrap().is_none());
    }

    /// The app hears a daemon of another protocol as such, and one that closes without a word
    /// as rejected, never as a daemon that isn't there.
    #[tokio::test]
    async fn hello_tells_a_mismatch_and_a_silent_close_apart() {
        let daemon = Daemon::start("hello");
        let paths = daemon.platform.paths().clone();
        let older = daemon.info(PROTOCOL_VERSION - 1);
        let answering = tokio::spawn({
            let paths = paths.clone();
            async move { hello(&paths, client()).await }
        });
        // A daemon of this change's kind answers an app of another protocol.
        let pending = daemon.listener.accept().await.unwrap();
        let Ok(Accepted::Client { mut connection, .. }) = pending.handshake(&daemon.token).await
        else {
            panic!("expected the app's hello");
        };
        connection
            .writer
            .write(&ServerFrame::ProtocolMismatch {
                daemon: older.clone(),
            })
            .await
            .unwrap();
        match answering.await.unwrap() {
            Ok(Hello::Mismatch { daemon, .. }) => assert_eq!(daemon, older),
            Ok(Hello::Welcome { .. }) => panic!("welcomed"),
            Err(err) => panic!("{err}"),
        }

        let answering = tokio::spawn({
            let paths = paths.clone();
            async move { connect_to(&paths, client()).await.map(|_| ()) }
        });
        let pending = daemon.listener.accept().await.unwrap();
        let Ok(Accepted::Client { mut connection, .. }) = pending.handshake(&daemon.token).await
        else {
            panic!("expected the app's hello");
        };
        connection
            .writer
            .write(&ServerFrame::ProtocolMismatch {
                daemon: older.clone(),
            })
            .await
            .unwrap();
        assert!(matches!(
            answering.await.unwrap(),
            Err(Error::ProtocolMismatch(daemon)) if *daemon == older
        ));

        // A daemon from before it drops the hello unanswered.
        let answering = tokio::spawn({
            let paths = paths.clone();
            async move { hello(&paths, client()).await.map(|_| ()) }
        });
        let pending = daemon.listener.accept().await.unwrap();
        drop(pending.handshake(&daemon.token).await);
        assert!(matches!(answering.await.unwrap(), Err(Error::Rejected)));
    }
}
