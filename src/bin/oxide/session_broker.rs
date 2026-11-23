//! Parent-owned transport for nested session membership and permit borrowing.

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod socket_root;
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests;
mod wire;

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::ffi::{CString, OsStr};
use std::fs::{self, File, Permissions};
use std::io::{self, BufRead, BufReader, Read};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wire::{Request, Response};

pub(super) const BROKER_ENV: &str = "OXIDE_SESSION_BROKER";
pub(super) const TOKEN_ENV: &str = "OXIDE_SESSION_TOKEN";
const SOCKET_NAME: &str = "s";
/// `sun_path` is the last field, so its capacity is the struct tail. That is
/// 104 bytes on Darwin and 108 on Linux, both including the terminator.
const SUN_PATH_BYTES: usize =
    std::mem::size_of::<libc::sockaddr_un>() - std::mem::offset_of!(libc::sockaddr_un, sun_path);
const MAX_SOCKET_PATH_BYTES: usize = SUN_PATH_BYTES - 1;
/// Platforms with the peer-credential check and the broker's Unix-socket
/// transport. Other Unix targets refuse before binding or joining.
pub(super) const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));
const MAX_CONNECTIONS: usize = 64;
const ACCEPT_INTERVAL: Duration = Duration::from_millis(10);
const AUTH_READ_TIMEOUT: Duration = Duration::from_secs(2);
const TOKEN_HEX_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PermitMode {
    Inherited,
    BorrowRequired,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Joined {
    pub session_id: String,
    pub member_id: u64,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub remaining_execution_ms: Option<u64>,
    pub permit_mode: PermitMode,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PermitGranted {
    pub borrow_id: u64,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub remaining_execution_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PermitDenial {
    Unavailable,
    Busy,
    DeadlineExpired,
}

/// A grant owns its share of the permit until the connection ends.
pub(super) type PermitGrant = Box<dyn Send>;

/// The broker calls this concurrently. A pending borrow must observe the
/// deadline and the connection, which is also how broker shutdown cancels it.
pub(super) trait PermitController: Send + Sync + 'static {
    fn borrow(&self, id: u64, wait: &BorrowWait<'_>) -> Result<PermitGrant, PermitDenial>;
}

pub(super) struct DenyBorrowing;

impl PermitController for DenyBorrowing {
    fn borrow(&self, _: u64, _: &BorrowWait<'_>) -> Result<PermitGrant, PermitDenial> {
        Err(PermitDenial::Unavailable)
    }
}

const BORROW_POLL: Duration = Duration::from_millis(50);

pub(super) struct BorrowWait<'a> {
    peer: &'a UnixStream,
    deadline: Option<Instant>,
}

impl BorrowWait<'_> {
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(super) fn check(&self) -> Result<(), PermitDenial> {
        self.check_after(Duration::ZERO)
    }

    pub(super) fn wait(&self) -> Result<(), PermitDenial> {
        self.check_after(BORROW_POLL)
    }

    fn check_after(&self, limit: Duration) -> Result<(), PermitDenial> {
        if socket_ready(self.peer, time_left(self.deadline, limit))
            .map_err(|_| PermitDenial::Unavailable)?
        {
            return Err(PermitDenial::Unavailable);
        }
        if remaining_ms(self.deadline) == Some(0) {
            return Err(PermitDenial::DeadlineExpired);
        }
        Ok(())
    }
}

fn time_left(deadline: Option<Instant>, limit: Duration) -> Duration {
    deadline.map_or(limit, |at| {
        limit.min(at.saturating_duration_since(Instant::now()))
    })
}

fn socket_ready(socket: &UnixStream, limit: Duration) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = i32::try_from(limit.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd and a bounded, nonnegative timeout.
    let ready = unsafe { libc::poll(&mut poll, 1, timeout) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(ready > 0)
}

struct SocketDirectory {
    path: PathBuf,
}

impl SocketDirectory {
    fn create(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        let template = root.join("ox.XXXXXX");
        check_socket_path(&template.join(SOCKET_NAME))?;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        socket_root::verify(&root)?;

        let mut name = CString::new(template.as_os_str().as_bytes())
            .map_err(|_| wire::invalid("session broker directory contains NUL"))?
            .into_bytes_with_nul();
        // SAFETY: The writable, NUL-terminated template ends in six X bytes.
        if unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        name.pop(); // mkdtemp replaces the suffix in place and preserves the terminator.
        let directory = Self {
            path: OsStr::from_bytes(&name).into(),
        };
        fs::set_permissions(&directory.path, Permissions::from_mode(0o700))?;
        Ok(directory)
    }

    fn socket(&self) -> PathBuf {
        self.path.join(SOCKET_NAME)
    }
}

impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.socket());
        let _ = fs::remove_dir(&self.path);
    }
}

fn check_socket_path(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute() || bytes.len() > MAX_SOCKET_PATH_BYTES || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "session broker requires an absolute socket path of at most {MAX_SOCKET_PATH_BYTES} bytes without NUL"
            ),
        ));
    }
    Ok(())
}

fn random_hex<const N: usize>() -> io::Result<String> {
    let mut bytes = [0u8; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(N * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    Ok(value)
}

/// The kernel's effective UID of the connected peer, read through the socket.
#[cfg(target_os = "macos")]
fn peer_uid(stream: &UnixStream) -> io::Result<libc::uid_t> {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: The socket is open, and both output pointers are valid.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> io::Result<libc::uid_t> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>())
        .expect("ucred fits in socklen_t");
    // SAFETY: The socket is open. `credentials` and `length` are valid for the
    // size the kernel is told, and SO_PEERCRED writes one `ucred` into them.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn check_peer(stream: &UnixStream) -> io::Result<()> {
    let uid = peer_uid(stream)?;
    // SAFETY: geteuid takes no arguments and has no preconditions.
    if uid != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "session broker peer has a different effective UID",
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn check_peer(_: &UnixStream) -> io::Result<()> {
    Err(unsupported())
}

/// Compare fixed-length tokens without an early exit on the first mismatch.
/// Token length is public: every token is exactly `TOKEN_HEX_BYTES` digits.
fn tokens_match(expected: &str, provided: &str) -> bool {
    if expected.len() != TOKEN_HEX_BYTES || provided.len() != TOKEN_HEX_BYTES {
        return false;
    }
    expected
        .bytes()
        .zip(provided.bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "session broker peer credentials are only supported on macOS and Linux",
    )
}

struct Session {
    id: String,
    token: String,
    permit_mode: PermitMode,
    members: AtomicUsize,
    permits: Box<dyn PermitController>,
}

/// Bound before launch and armed once the outer execution start is observed.
pub(super) struct SessionBroker {
    directory: SocketDirectory,
    session: Arc<Session>,
    control: Option<Sender<Option<Instant>>>,
    accept_thread: Option<JoinHandle<()>>,
    armed: bool,
}

impl SessionBroker {
    /// Bind before spawning a leased child. Unsupported Unix targets get none.
    pub(super) fn for_child(root: &Path, command: &mut Command) -> Result<Option<Self>, String> {
        if !SUPPORTED {
            return Ok(None);
        }
        Self::for_permits(root, command, PermitMode::Inherited, DenyBorrowing).map(Some)
    }

    pub(super) fn for_permits(
        root: &Path,
        command: &mut Command,
        mode: PermitMode,
        permits: impl PermitController,
    ) -> Result<Self, String> {
        let broker = Self::bind(root, mode, permits)
            .map_err(|error| format!("cannot start the Oxide session broker: {error}"))?;
        broker.apply(command);
        Ok(broker)
    }

    /// Bind inside a caller-selected temporary directory with trusted ancestors.
    pub(super) fn bind(
        root: &Path,
        permit_mode: PermitMode,
        permits: impl PermitController,
    ) -> io::Result<Self> {
        if !SUPPORTED {
            return Err(unsupported());
        }
        let directory = SocketDirectory::create(root)?;
        let listener = UnixListener::bind(directory.socket())?;
        listener.set_nonblocking(true)?;
        let session = Arc::new(Session {
            id: random_hex::<16>()?,
            token: random_hex::<32>()?,
            permit_mode,
            members: AtomicUsize::new(0),
            permits: Box::new(permits),
        });
        let (control, commands) = mpsc::channel();
        let server_session = Arc::clone(&session);
        let accept_thread = thread::Builder::new()
            .name("oxide-session".into())
            .spawn(move || accept_connections(listener, commands, server_session))?;
        Ok(Self {
            directory,
            session,
            control: Some(control),
            accept_thread: Some(accept_thread),
            armed: false,
        })
    }

    /// Advertise the endpoint privately to a command and its descendants.
    pub(super) fn apply(&self, command: &mut Command) {
        command
            .env(BROKER_ENV, self.directory.socket())
            .env(TOKEN_ENV, &self.session.token);
    }

    /// Enable handshakes with the outer command's absolute deadline, if bounded.
    pub(super) fn arm(&mut self, execution_deadline: Option<Instant>) -> io::Result<()> {
        if self.armed {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "session broker execution deadline is already armed",
            ));
        }
        self.control
            .as_ref()
            .expect("broker owns the control channel until drop")
            .send(execution_deadline)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "session broker has ended"))?;
        self.armed = true;
        Ok(())
    }

    /// Snapshot of live joined connections.
    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    pub(super) fn member_count(&self) -> usize {
        self.session.members.load(Ordering::Relaxed)
    }
}

impl Drop for SessionBroker {
    fn drop(&mut self) {
        self.control.take();
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
    }
}

struct Connection {
    socket: UnixStream,
    thread: JoinHandle<()>,
}

fn accept_connections(
    listener: UnixListener,
    commands: Receiver<Option<Instant>>,
    session: Arc<Session>,
) {
    let Ok(deadline) = commands.recv() else {
        return;
    };
    let mut connections: Vec<Connection> = Vec::new();
    let mut next_id = 1u64;
    while commands.try_recv() == Err(TryRecvError::Empty) {
        while let Some(index) = connections
            .iter()
            .position(|entry| entry.thread.is_finished())
        {
            let _ = connections.swap_remove(index).thread.join();
        }
        match listener.accept() {
            Ok((stream, _)) if connections.len() < MAX_CONNECTIONS => {
                let Ok(socket) = stream.try_clone() else {
                    continue;
                };
                let id = next_id;
                let Some(successor) = id.checked_add(1) else {
                    break;
                };
                next_id = successor;
                let session = Arc::clone(&session);
                if let Ok(thread) = thread::Builder::new()
                    .name("oxide-session-peer".into())
                    .spawn(move || {
                        let _ = serve(stream, &session, id, deadline);
                    })
                {
                    connections.push(Connection { socket, thread });
                }
            }
            Ok(_) => {} // Refuse excess connections without allocating workers.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if commands.recv_timeout(ACCEPT_INTERVAL) != Err(RecvTimeoutError::Timeout) {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    drop(listener);
    for entry in &connections {
        let _ = entry.socket.shutdown(Shutdown::Both);
    }
    for entry in connections {
        let _ = entry.thread.join();
    }
}

fn remaining_ms(deadline: Option<Instant>) -> Option<u64> {
    deadline.map(|at| {
        u64::try_from(at.saturating_duration_since(Instant::now()).as_millis()).unwrap_or(u64::MAX)
    })
}

struct Member<'a>(&'a AtomicUsize);

impl<'a> Member<'a> {
    fn join(members: &'a AtomicUsize) -> Self {
        members.fetch_add(1, Ordering::Relaxed);
        Self(members)
    }
}

impl Drop for Member<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn serve(
    stream: UnixStream,
    session: &Session,
    id: u64,
    deadline: Option<Instant>,
) -> io::Result<()> {
    // The accept thread holds a duplicate to interrupt blocking I/O at shutdown.
    // Shut down here as well so refusal or release is visible before reaping.
    let mut reader = BufReader::new(&stream);
    // Accepted sockets may inherit nonblocking mode from the listener.
    let result = stream
        .set_nonblocking(false)
        .and_then(|()| serve_request(&mut reader, session, id, deadline));
    let _ = stream.shutdown(Shutdown::Both);
    result
}

fn serve_request(
    reader: &mut BufReader<&UnixStream>,
    session: &Session,
    id: u64,
    deadline: Option<Instant>,
) -> io::Result<()> {
    let stream = *reader.get_ref();
    check_peer(stream)?;
    stream.set_read_timeout(Some(AUTH_READ_TIMEOUT))?;
    let received_at = Instant::now();
    let Some(request) = wire::read::<Request>(reader)? else {
        return Ok(());
    };
    stream.set_read_timeout(None)?;

    match request {
        Request::Join { token } if tokens_match(&session.token, &token) => {
            let member = Member::join(&session.members);
            wire::write(
                &mut &*stream,
                Response::Joined(Joined {
                    session_id: session.id.clone(),
                    member_id: id,
                    remaining_execution_ms: remaining_ms(deadline),
                    permit_mode: session.permit_mode,
                }),
            )?;
            match wire::read(reader)? {
                None => Ok(()),
                Some(Request::Release { member_id }) if member_id == id => {
                    drop(member);
                    wire::write(&mut &*stream, Response::Released {})
                }
                _ => Err(wire::invalid("unexpected session member request")),
            }
        }
        Request::BorrowPermit {
            token,
            admission_ms,
        } if tokens_match(&session.token, &token) => {
            let admission = reported_deadline(received_at, admission_ms)?;
            let wait = BorrowWait {
                peer: stream,
                deadline: admission.into_iter().chain(deadline).min(),
            };
            if reader.buffer().is_empty() {
                serve_borrow(reader, session.permits.as_ref(), id, deadline, &wait)
            } else {
                Err(wire::invalid("unexpected data after the permit request"))
            }
        }
        _ => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "session broker authentication refused",
        )),
    }
}

fn serve_borrow(
    reader: &mut BufReader<&UnixStream>,
    permits: &dyn PermitController,
    id: u64,
    deadline: Option<Instant>,
    wait: &BorrowWait<'_>,
) -> io::Result<()> {
    let mut stream = *reader.get_ref();
    let _grant = match wait
        .check()
        .and_then(|()| permits.borrow(id, wait))
        .and_then(|grant| {
            wait.check()?;
            Ok(grant)
        }) {
        Ok(grant) => grant,
        Err(reason) => {
            if socket_ready(stream, Duration::ZERO)? {
                return Err(wire::invalid(
                    "permit borrower disconnected or sent unexpected data",
                ));
            }
            return wire::write(&mut stream, Response::PermitDenied { reason });
        }
    };
    wire::write(
        &mut stream,
        Response::PermitGranted(PermitGranted {
            borrow_id: id,
            remaining_execution_ms: remaining_ms(deadline),
        }),
    )?;
    if !reader.fill_buf()?.is_empty() {
        return Err(wire::invalid("unexpected permit borrower request"));
    }
    Ok(())
}

/// A live grant. Closing the socket releases its membership or borrowed permit.
#[derive(Debug)]
pub(super) struct SessionClient {
    reader: BufReader<UnixStream>,
    member_id: Option<u64>,
}

#[derive(Debug)]
pub(super) enum BorrowOutcome {
    Granted {
        client: SessionClient,
        permit: PermitGranted,
    },
    Denied {
        reason: PermitDenial,
    },
}

fn reported_deadline(sent: Instant, remaining_ms: Option<u64>) -> io::Result<Option<Instant>> {
    remaining_ms
        .map(|ms| {
            sent.checked_add(Duration::from_millis(ms))
                .ok_or_else(|| wire::invalid("invalid outer Oxide deadline"))
        })
        .transpose()
}

impl SessionClient {
    fn live(reader: BufReader<UnixStream>, member_id: Option<u64>) -> io::Result<Self> {
        reader.get_ref().set_read_timeout(None)?;
        Ok(Self { reader, member_id })
    }

    fn open(path: &Path, request: Request) -> io::Result<BufReader<UnixStream>> {
        check_socket_path(path)?;
        let mut stream = UnixStream::connect(path)?;
        check_peer(&stream)?;
        stream.set_read_timeout(Some(AUTH_READ_TIMEOUT))?;
        stream.set_write_timeout(Some(AUTH_READ_TIMEOUT))?;
        wire::write(&mut stream, request)?;
        Ok(BufReader::new(stream))
    }

    fn read_response(reader: &mut BufReader<UnixStream>) -> io::Result<Response> {
        wire::read(reader)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "session broker closed the connection",
            )
        })
    }

    /// Join using the endpoint inherited from the parent.
    pub(super) fn join(path: &Path, token: &str) -> io::Result<(Self, Joined)> {
        let mut reader = Self::open(
            path,
            Request::Join {
                token: token.into(),
            },
        )?;
        let Response::Joined(joined) = Self::read_response(&mut reader)? else {
            return Err(wire::invalid("expected a session join response"));
        };
        let client = Self::live(reader, Some(joined.member_id))?;
        Ok((client, joined))
    }

    /// Request a permit on a separate, connection-scoped grant.
    fn borrow_permit(
        path: &Path,
        token: &str,
        deadline: Option<Instant>,
        mut check: impl FnMut() -> io::Result<()>,
    ) -> io::Result<BorrowOutcome> {
        let mut reader = Self::open(
            path,
            Request::BorrowPermit {
                token: token.into(),
                admission_ms: remaining_ms(deadline),
            },
        )?;
        loop {
            check()?;
            if remaining_ms(deadline) == Some(0) {
                return Ok(BorrowOutcome::Denied {
                    reason: PermitDenial::DeadlineExpired,
                });
            }
            if socket_ready(reader.get_ref(), time_left(deadline, BORROW_POLL))? {
                check()?;
                break;
            }
        }
        match Self::read_response(&mut reader)? {
            Response::PermitGranted(permit) => Ok(BorrowOutcome::Granted {
                client: Self::live(reader, None)?,
                permit,
            }),
            Response::PermitDenied { reason } => Ok(BorrowOutcome::Denied { reason }),
            _ => Err(wire::invalid("expected a session permit response")),
        }
    }

    /// Release a member with acknowledgement, or close a borrowed permit socket.
    pub(super) fn release(mut self) -> io::Result<()> {
        if let Some(member_id) = self.member_id {
            self.reader
                .get_ref()
                .set_read_timeout(Some(AUTH_READ_TIMEOUT))?;
            wire::write(&mut self.reader.get_ref(), Request::Release { member_id })?;
            if !matches!(
                Self::read_response(&mut self.reader)?,
                Response::Released {}
            ) {
                return Err(wire::invalid("expected a session release acknowledgement"));
            }
        }
        Ok(())
    }

    /// Wait for EOF from the parent. Any read error also invalidates the grant.
    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    pub(super) fn wait_for_end(mut self) -> io::Result<()> {
        if !self.reader.fill_buf()?.is_empty() {
            return Err(wire::invalid("unexpected session broker message"));
        }
        Ok(())
    }

    fn check_live(&self) -> io::Result<()> {
        if self.reader.buffer().is_empty() && !socket_ready(self.reader.get_ref(), Duration::ZERO)?
        {
            Ok(())
        } else {
            Err(wire::invalid("socket closed or sent unexpected data"))
        }
    }
}

#[derive(Clone)]
pub(super) struct PermitRelay {
    path: PathBuf,
    token: String,
}

impl PermitController for PermitRelay {
    fn borrow(&self, _: u64, wait: &BorrowWait<'_>) -> Result<PermitGrant, PermitDenial> {
        let response = SessionClient::borrow_permit(&self.path, &self.token, wait.deadline, || {
            wait.check()
                .map_err(|_| io::Error::from(io::ErrorKind::Interrupted))
        });
        wait.check()?;
        match response.map_err(|_| PermitDenial::Unavailable)? {
            BorrowOutcome::Granted { client, .. } => Ok(Box::new(client)),
            BorrowOutcome::Denied { reason } => Err(reason),
        }
    }
}

pub(super) enum BorrowStop {
    Admission,
    Signal(i32),
    Failed(String),
}

/// A command's authenticated membership and conservative local deadline.
pub(super) struct JoinedSession {
    client: SessionClient,
    relay: Option<PermitRelay>,
    permit: RefCell<Option<SessionClient>>,
    pub(super) deadline: Option<Instant>,
}

impl JoinedSession {
    pub(super) fn from_env() -> Result<Option<Self>, String> {
        let endpoint = std::env::var_os(BROKER_ENV);
        let token = std::env::var_os(TOKEN_ENV);
        let (path, token) = match (endpoint.as_deref(), token.as_deref()) {
            (None, None) => return Ok(None),
            (Some(path), Some(token)) => (Path::new(path), token),
            _ => {
                return Err(format!(
                    "nested Oxide requires both {BROKER_ENV} and {TOKEN_ENV}"
                ))
            }
        };
        if !SUPPORTED {
            return Err(format!(
                "cannot join the outer Oxide session: {}",
                unsupported()
            ));
        }
        let token = token
            .to_str()
            .filter(|token| {
                token.len() == TOKEN_HEX_BYTES
                    && token
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            })
            .ok_or("invalid outer Oxide session token")?;
        // Anchor before sending Join so time in transit can only shorten the
        // advertised interval. A descendant cannot move this deadline later.
        let sent = Instant::now();
        let (client, joined) = SessionClient::join(path, token)
            .map_err(|error| format!("cannot join the outer Oxide session: {error}"))?;
        let deadline = reported_deadline(sent, joined.remaining_execution_ms)
            .map_err(|error| error.to_string())?;
        let relay = (joined.permit_mode == PermitMode::BorrowRequired).then(|| PermitRelay {
            path: path.into(),
            token: token.into(),
        });
        Ok(Some(Self {
            client,
            relay,
            permit: RefCell::new(None),
            deadline,
        }))
    }

    pub(super) fn relay(&self) -> Option<PermitRelay> {
        self.relay.clone()
    }

    pub(super) fn borrow(&self) -> Result<(), BorrowStop> {
        let Some(relay) = &self.relay else {
            return Ok(());
        };
        let response = SessionClient::borrow_permit(
            &relay.path,
            &relay.token,
            crate::budget::admission_deadline(),
            || {
                self.client.check_live()?;
                if crate::process::received_signal().is_some() {
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                Ok(())
            },
        );
        if crate::budget::admission_expired() {
            return Err(BorrowStop::Admission);
        }
        if let Some(signal) = crate::process::received_signal() {
            return Err(BorrowStop::Signal(signal));
        }
        match response {
            Ok(BorrowOutcome::Granted { client, permit }) => {
                // The Join anchored this same outer clock, before the wait.
                // A grant never starts a fresh clock.
                if permit.remaining_execution_ms == Some(0) || crate::budget::admission_expired() {
                    return Err(BorrowStop::Admission);
                }
                *self.permit.borrow_mut() = Some(client);
                Ok(())
            }
            Ok(BorrowOutcome::Denied {
                reason: PermitDenial::DeadlineExpired,
            }) => Err(BorrowStop::Admission),
            Ok(BorrowOutcome::Denied { reason }) => Err(BorrowStop::Failed(format!(
                "the outer Oxide session denied the build permit ({reason:?})"
            ))),
            Err(error) => Err(BorrowStop::Failed(format!(
                "cannot borrow an Oxide build permit: {error}"
            ))),
        }
    }

    pub(super) fn return_permit(&self) {
        self.permit.borrow_mut().take();
    }

    pub(super) fn check_live(&self) -> Result<(), String> {
        let check = self.client.check_live().and_then(|()| {
            self.permit
                .borrow()
                .as_ref()
                .map_or(Ok(()), SessionClient::check_live)
        });
        check.map_err(|error| format!("the outer Oxide session: {error}"))
    }

    pub(super) fn release(self) -> io::Result<()> {
        self.return_permit();
        self.client.release()
    }
}
