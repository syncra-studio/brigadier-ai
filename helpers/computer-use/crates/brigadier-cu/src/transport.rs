//! How the shim, the CLI and Brigadier reach the daemon, and how the daemon
//! listens for them.
//!
//! macOS and Linux: a Unix socket in the helper's private home (`cu.sock`),
//! bound and checked by `serve`. Windows: a named pipe that only the user's
//! own processes may open, named after that same path ([`pipe_name`]), so
//! each helper home has its own daemon as it does elsewhere. Either way a
//! request and its answer are one JSON line each.
//!
//! [`connect`] and [`Stream::connect`] are the client side for every caller:
//! the MCP shim, the CLI, `host` requests and the supervisor's probe.

use std::io;
#[cfg(windows)]
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite};

/// The reading half of a connection.
pub type Reader = Box<dyn AsyncRead + Send + Unpin>;
/// The writing half of a connection.
pub type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// The named pipe that stands for the socket path `socket_path` on Windows:
/// `\\.\pipe\brigadier-cu-<16 hex digits of its SHA-256>`. The path is
/// compared case-insensitively, as Windows paths are.
#[cfg_attr(unix, allow(dead_code))]
pub fn pipe_name(socket_path: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(socket_path.to_lowercase().as_bytes());
    let hex: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(r"\\.\pipe\brigadier-cu-{hex}")
}

/// How long a client waits for a free pipe instance while the daemon is
/// busy creating the next one.
#[cfg(windows)]
const PIPE_BUSY_WAIT: Duration = Duration::from_secs(3);

/// `ERROR_PIPE_BUSY`: every instance of the pipe is taken right now.
#[cfg(windows)]
const ERROR_PIPE_BUSY: i32 = 231;

/// Connect to the daemon for `socket_path`, for async callers (the shim).
pub async fn connect(socket_path: &str) -> io::Result<(Reader, Writer)> {
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(socket_path).await?;
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            validate_unix_server(stream.as_raw_fd())?;
        }
        let (reader, writer) = stream.into_split();
        Ok((Box::new(reader), Box::new(writer)))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        let name = pipe_name(socket_path);
        let until = Instant::now() + PIPE_BUSY_WAIT;
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => break client,
                Err(error)
                    if error.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < until =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => return Err(error),
            }
        };
        use std::os::windows::io::AsRawHandle;
        pipe_server::validate_server(client.as_raw_handle())?;
        let (reader, writer) = tokio::io::split(client);
        Ok((Box::new(reader), Box::new(writer)))
    }
}

/// A blocking connection to the daemon, for one request at a time. Unix
/// callers use their socket directly; Windows's pipe goes through this.
#[cfg_attr(unix, allow(dead_code))]
pub struct Stream {
    #[cfg(unix)]
    inner: std::os::unix::net::UnixStream,
    #[cfg(windows)]
    inner: std::fs::File,
}

#[cfg_attr(unix, allow(dead_code))]
impl Stream {
    /// Connect to the daemon for `socket_path`.
    pub fn connect(socket_path: &str) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let inner = std::os::unix::net::UnixStream::connect(socket_path)?;
            #[cfg(target_os = "linux")]
            {
                use std::os::fd::AsRawFd;
                validate_unix_server(inner.as_raw_fd())?;
            }
            Ok(Self { inner })
        }
        #[cfg(windows)]
        {
            let name = pipe_name(socket_path);
            let until = Instant::now() + PIPE_BUSY_WAIT;
            loop {
                match std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&name)
                {
                    Ok(inner) => {
                        use std::os::windows::io::AsRawHandle;
                        pipe_server::validate_server(inner.as_raw_handle())?;
                        return Ok(Self { inner });
                    }
                    Err(error)
                        if error.raw_os_error() == Some(ERROR_PIPE_BUSY)
                            && Instant::now() < until =>
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    /// The read and write timeouts. A named pipe opened as a file has none:
    /// on Windows reads and writes block, and callers that need a deadline
    /// run the exchange on a thread of its own ([`exchange_within`]).
    pub fn set_timeouts(
        &self,
        read: Option<std::time::Duration>,
        write: Option<std::time::Duration>,
    ) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.inner.set_read_timeout(read)?;
            self.inner.set_write_timeout(write)
        }
        #[cfg(windows)]
        {
            let _ = (read, write);
            Ok(())
        }
    }
}

/// Authenticate the server before sending it any capability token. A
/// predictable socket/pipe name is not evidence of the server's identity.
#[cfg(target_os = "linux")]
pub(crate) fn validate_unix_server(fd: std::os::fd::RawFd) -> io::Result<()> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: a live socket descriptor and correctly sized writable buffer.
    let status = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    if credentials.uid != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "computer-use server runs as another user",
        ));
    }
    crate::peer_auth::validate(u32::try_from(credentials.pid).ok())
}

impl io::Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buffer)
    }
}

impl io::Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.write(buffer)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Send one request line on a fresh connection and read one answer line,
/// giving up after `deadline`. `None` when nothing listens; `Some(Err)` when
/// something accepted but didn't answer in time.
#[cfg_attr(unix, allow(dead_code))]
pub fn exchange_within(
    socket_path: &str,
    line: &str,
    deadline: std::time::Duration,
) -> Option<io::Result<Vec<u8>>> {
    let stream = Stream::connect(socket_path).ok()?;
    #[cfg(unix)]
    {
        Some(exchange(stream, line, Some(deadline)))
    }
    #[cfg(windows)]
    {
        // A pipe opened as a file has no timeouts: run the exchange on its own
        // thread and stop waiting at the deadline. A thread left blocked ends
        // when the daemon closes the pipe or the process exits.
        let (sender, receiver) = std::sync::mpsc::channel();
        let line = line.to_owned();
        std::thread::spawn(move || {
            let _ = sender.send(exchange(stream, &line, None));
        });
        Some(receiver.recv_timeout(deadline).unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the daemon did not answer in time",
            ))
        }))
    }
}

/// Write `line` and read up to the first newline of the answer.
#[cfg_attr(unix, allow(dead_code))]
fn exchange(
    mut stream: Stream,
    line: &str,
    deadline: Option<std::time::Duration>,
) -> io::Result<Vec<u8>> {
    use io::{Read, Write};
    let until = deadline.map(|deadline| std::time::Instant::now() + deadline);
    stream.set_timeouts(None, deadline)?;
    stream.write_all(format!("{line}\n").as_bytes())?;
    let mut answer = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        if let Some(until) = until {
            let remaining = until.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer in time"));
            }
            stream.set_timeouts(Some(remaining), deadline)?;
        }
        match stream.read(&mut chunk) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the daemon closed the connection without an answer",
                ))
            }
            Ok(read) => {
                answer.extend_from_slice(&chunk[..read]);
                if let Some(end) = answer.iter().position(|&byte| byte == b'\n') {
                    answer.truncate(end);
                    return Ok(answer);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
pub use pipe_server::PipeListener;

/// The daemon's side on Windows: pipe instances private to the user, each
/// client checked before its first request is read.
#[cfg(windows)]
mod pipe_server {
    use std::io;
    use std::os::windows::io::AsRawHandle;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    use super::{Reader, Writer};

    pub(super) fn validate_server(handle: std::os::windows::io::RawHandle) -> io::Result<()> {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;
        let mut pid = 0;
        // SAFETY: the caller owns a live connected pipe handle.
        unsafe { GetNamedPipeServerProcessId(HANDLE(handle), &mut pid) }
            .map_err(io::Error::other)?;
        if !matches!((security::current_user_sid(), security::process_user_sid(pid)),
            (Some(own), Some(peer)) if own.eq_ignore_ascii_case(&peer))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "computer-use server runs as another user",
            ));
        }
        crate::peer_auth::validate(Some(pid))
    }

    /// Consecutive failures to create or connect an instance before the
    /// daemon gives up, so its supervisor can start a fresh one.
    const INSTANCE_FAILURE_LIMIT: u32 = 20;

    /// The pipe the daemon serves.
    pub struct PipeListener {
        name: String,
        /// The security descriptor every instance is created with: full access
        /// for the user's own SID only. Kept for the daemon's lifetime.
        security: security::Attributes,
        /// The instance waiting for the next client.
        next: NamedPipeServer,
        failures: u32,
    }

    /// One accepted client.
    pub struct PipeConnection {
        server: NamedPipeServer,
        client_pid: Option<u32>,
    }

    impl PipeConnection {
        /// The client process, as the pipe reports it.
        pub fn client_pid(&self) -> Option<u32> {
            self.client_pid
        }

        /// The two halves, like a socket's `into_split`.
        pub fn into_split(self) -> (Reader, Writer) {
            let (reader, writer) = tokio::io::split(self.server);
            (Box::new(reader), Box::new(writer))
        }
    }

    impl PipeListener {
        /// Create the pipe's first instance. Fails when another daemon owns
        /// the name, so two daemons never serve one home.
        pub fn bind(name: &str) -> anyhow::Result<Self> {
            let security = security::Attributes::current_user_only()
                .ok_or_else(|| anyhow::anyhow!("could not build the pipe's access rules"))?;
            let next = create(name, &security, true)
                .map_err(|error| anyhow::anyhow!("create named pipe {name}: {error}"))?;
            Ok(Self {
                name: name.to_owned(),
                security,
                next,
                failures: 0,
            })
        }

        /// Wait for the next client of the user's own: a client running as
        /// anyone else is disconnected before anything is read. The second
        /// value mirrors a socket's peer address and carries nothing.
        pub async fn accept(&mut self) -> io::Result<(PipeConnection, ())> {
            loop {
                if let Err(error) = self.next.connect().await {
                    self.fail(&error)?;
                    self.next = self.fresh_instance().await?;
                    continue;
                }
                let replacement = self.fresh_instance().await?;
                let server = std::mem::replace(&mut self.next, replacement);
                self.failures = 0;
                let handle = server.as_raw_handle();
                let client_pid = security::client_process_id(handle);
                let same_user = matches!(
                    (security::current_user_sid(), client_pid.and_then(security::process_user_sid)),
                    (Some(own), Some(client)) if own.eq_ignore_ascii_case(&client)
                );
                if !same_user {
                    eprintln!("brigadier-cu: refused a pipe client running as another user");
                    let _ = server.disconnect();
                    continue;
                }
                return Ok((PipeConnection { server, client_pid }, ()));
            }
        }

        async fn fresh_instance(&mut self) -> io::Result<NamedPipeServer> {
            loop {
                match create(&self.name, &self.security, false) {
                    Ok(server) => return Ok(server),
                    Err(error) => {
                        self.fail(&error)?;
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }

        fn fail(&mut self, error: &io::Error) -> io::Result<()> {
            self.failures += 1;
            eprintln!(
                "brigadier-cu: named pipe {} failed ({} in a row): {error}",
                self.name, self.failures
            );
            if self.failures >= INSTANCE_FAILURE_LIMIT {
                return Err(io::Error::other(format!(
                    "named pipe {}: {} failures in a row",
                    self.name, self.failures
                )));
            }
            Ok(())
        }
    }

    fn create(
        name: &str,
        security: &security::Attributes,
        first: bool,
    ) -> io::Result<NamedPipeServer> {
        // SAFETY: the attributes and the descriptor they point to live as
        // long as the listener, longer than this call.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, security.as_ptr())
        }
    }

    /// The Windows security calls the pipe needs, after the driver's own
    /// daemon (cua-driver `serve.rs`).
    mod security {
        use std::ffi::c_void;

        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
        use windows::Win32::Security::Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        };
        use windows::Win32::Security::{
            GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER,
        };
        use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
        use windows::Win32::System::Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        /// `SECURITY_ATTRIBUTES` with a descriptor that gives the user's own
        /// SID full access and nobody else any, with a low integrity label so
        /// a client at another integrity level of the same user still connects.
        pub struct Attributes {
            attributes: Box<SECURITY_ATTRIBUTES>,
        }

        // SAFETY: the descriptor is only read after creation, by the kernel.
        unsafe impl Send for Attributes {}
        unsafe impl Sync for Attributes {}

        impl Attributes {
            pub fn current_user_only() -> Option<Self> {
                let sid = current_user_sid()?;
                let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})S:(ML;;NW;;;LW)\0")
                    .encode_utf16()
                    .collect();
                let mut descriptor = PSECURITY_DESCRIPTOR::default();
                // SAFETY: `sddl` is NUL-terminated and outlives the call; the
                // descriptor is kept (never freed) for the daemon's lifetime.
                unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        PCWSTR(sddl.as_ptr()),
                        SDDL_REVISION_1,
                        &mut descriptor,
                        None,
                    )
                    .ok()?;
                }
                Some(Self {
                    attributes: Box::new(SECURITY_ATTRIBUTES {
                        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                        lpSecurityDescriptor: descriptor.0,
                        bInheritHandle: false.into(),
                    }),
                })
            }

            pub fn as_ptr(&self) -> *mut c_void {
                (&*self.attributes as *const SECURITY_ATTRIBUTES)
                    .cast_mut()
                    .cast()
            }
        }

        /// The process at the other end of a pipe instance.
        pub fn client_process_id(pipe: std::os::windows::io::RawHandle) -> Option<u32> {
            let mut pid = 0u32;
            // SAFETY: `pipe` is a live pipe handle owned by the caller.
            unsafe { GetNamedPipeClientProcessId(HANDLE(pipe), &mut pid) }.ok()?;
            (pid != 0).then_some(pid)
        }

        /// This process's user SID, as a string (`S-1-5-21-…`).
        pub fn current_user_sid() -> Option<String> {
            // SAFETY: the pseudo handle of the current process needs no close.
            token_user_sid(unsafe { GetCurrentProcess() })
        }

        /// The user SID of process `pid`.
        pub fn process_user_sid(pid: u32) -> Option<String> {
            // SAFETY: the handle is checked and closed.
            unsafe {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
                let sid = token_user_sid(process);
                let _ = CloseHandle(process);
                sid
            }
        }

        fn token_user_sid(process: HANDLE) -> Option<String> {
            // SAFETY: every handle is checked and closed; the buffer is sized
            // by the first call and read unaligned.
            unsafe {
                let mut token = HANDLE::default();
                OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
                let mut required = 0u32;
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut required);
                if required == 0 {
                    let _ = CloseHandle(token);
                    return None;
                }
                let mut buffer = vec![0u8; required as usize];
                let read = GetTokenInformation(
                    token,
                    TokenUser,
                    Some(buffer.as_mut_ptr().cast()),
                    required,
                    &mut required,
                );
                let _ = CloseHandle(token);
                read.ok()?;
                let user = std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>());
                let mut text = PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
                let sid = text.to_string().ok();
                let _ = LocalFree(HLOCAL(text.0.cast()));
                sid.filter(|sid| !sid.is_empty())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pipe_name;

    #[test]
    fn each_home_has_its_own_pipe() {
        let a = pipe_name(r"C:\Users\me\AppData\Roaming\Brigadier\computer\cu.sock");
        let b = pipe_name(r"C:\Users\me\AppData\Roaming\Brigadier Dev\computer\cu.sock");
        assert!(a.starts_with(r"\\.\pipe\brigadier-cu-"));
        assert_eq!(a.len(), r"\\.\pipe\brigadier-cu-".len() + 16);
        assert_ne!(a, b);
        // Windows paths compare without case.
        assert_eq!(
            a,
            pipe_name(r"c:\users\ME\appdata\roaming\brigadier\computer\CU.SOCK")
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_clients_authenticate_the_kernel_server_identity() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("test.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        // Both clients verify the peer before writing anything to it.
        assert!(super::Stream::connect(socket.to_str().unwrap()).is_ok());
        assert!(super::connect(socket.to_str().unwrap()).await.is_ok());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_pipe_checks_the_server_and_client_before_exchange() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let path = format!("transport-test-path-{}", uuid::Uuid::new_v4());
        let mut listener = super::PipeListener::bind(&super::pipe_name(&path)).unwrap();
        let server = tokio::spawn(async move {
            let (connection, ()) = listener.accept().await.unwrap();
            assert_eq!(connection.client_pid(), Some(std::process::id()));
            let (mut reader, mut writer) = connection.into_split();
            assert_eq!(reader.read_u8().await.unwrap(), b'A');
            writer.write_u8(b'B').await.unwrap();
        });
        let (mut reader, mut writer) = super::connect(&path).await.unwrap();
        writer.write_u8(b'A').await.unwrap();
        assert_eq!(reader.read_u8().await.unwrap(), b'B');
        server.await.unwrap();
    }
}
