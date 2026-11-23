//! Parent-owned transport for nested session membership and permit borrowing.

#![allow(
    dead_code,
    reason = "Q-020 wires the broker into command entry points in later slices"
)]

#[cfg(all(test, target_os = "macos"))]
mod tests;
mod wire;

use serde::{Deserialize, Serialize};
use std::ffi::{CString, OsStr};
use std::fs::{self, File, Permissions};
use std::io::{self, BufRead, BufReader, Read};
use std::net::Shutdown;
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
const MAX_SOCKET_PATH_BYTES: usize = 103; // Darwin sun_path, including its terminator, is 104.
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

/// Supplies permit policy and owns actual permit custody. Both callbacks must
/// return promptly. The broker calls them concurrently and drains them on drop.
pub(super) trait PermitController: Send + Sync + 'static {
    fn try_borrow(&self, borrow_id: u64) -> Result<(), PermitDenial>;
    fn release(&self, borrow_id: u64);
}

pub(super) struct DenyBorrowing;

impl PermitController for DenyBorrowing {
    fn try_borrow(&self, _: u64) -> Result<(), PermitDenial> {
        Err(PermitDenial::Unavailable)
    }

    fn release(&self, _: u64) {}
}

struct SocketDirectory {
    path: PathBuf,
}

impl SocketDirectory {
    fn create(root: &Path) -> io::Result<Self> {
        let template = root.canonicalize()?.join("ox.XXXXXX");
        check_socket_path(&template.join(SOCKET_NAME))?;

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
            "session broker requires an absolute socket path of at most 103 bytes without NUL",
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

#[cfg(target_os = "macos")]
fn check_peer(stream: &UnixStream) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: The socket is open, and both output pointers are valid.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: geteuid takes no arguments and has no preconditions.
    if uid != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "session broker peer has a different effective UID",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn check_peer(_: &UnixStream) -> io::Result<()> {
    Err(unsupported())
}

#[cfg(target_os = "macos")]
fn tokens_match(expected: &str, provided: &str) -> bool {
    unsafe extern "C" {
        fn timingsafe_bcmp(
            left: *const libc::c_void,
            right: *const libc::c_void,
            len: libc::size_t,
        ) -> libc::c_int;
    }

    if expected.len() != TOKEN_HEX_BYTES || provided.len() != TOKEN_HEX_BYTES {
        return false;
    }
    // SAFETY: Both strings have at least TOKEN_HEX_BYTES readable bytes.
    unsafe {
        timingsafe_bcmp(
            expected.as_ptr().cast(),
            provided.as_ptr().cast(),
            TOKEN_HEX_BYTES,
        ) == 0
    }
}

#[cfg(not(target_os = "macos"))]
fn tokens_match(_: &str, _: &str) -> bool {
    false
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "session broker peer credentials are only supported on macOS",
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
    /// Bind inside a caller-selected temporary directory with trusted ancestors.
    pub(super) fn bind(
        root: &Path,
        permit_mode: PermitMode,
        permits: impl PermitController,
    ) -> io::Result<Self> {
        if !cfg!(target_os = "macos") {
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

struct Borrowed<'a> {
    controller: &'a dyn PermitController,
    id: u64,
}

impl Drop for Borrowed<'_> {
    fn drop(&mut self) {
        self.controller.release(self.id);
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
        Request::BorrowPermit { token } if tokens_match(&session.token, &token) => {
            serve_borrow(reader, session.permits.as_ref(), id, deadline)
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
) -> io::Result<()> {
    let mut stream = *reader.get_ref();
    if remaining_ms(deadline) == Some(0) {
        return wire::write(
            &mut stream,
            Response::PermitDenied {
                reason: PermitDenial::DeadlineExpired,
            },
        );
    }
    if let Err(reason) = permits.try_borrow(id) {
        return wire::write(&mut stream, Response::PermitDenied { reason });
    }

    let borrow = Borrowed {
        controller: permits,
        id,
    };
    let remaining_execution_ms = remaining_ms(deadline);
    if remaining_execution_ms == Some(0) {
        drop(borrow);
        return wire::write(
            &mut stream,
            Response::PermitDenied {
                reason: PermitDenial::DeadlineExpired,
            },
        );
    }
    wire::write(
        &mut stream,
        Response::PermitGranted(PermitGranted {
            borrow_id: id,
            remaining_execution_ms,
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

impl SessionClient {
    fn open(path: &Path, request: Request) -> io::Result<(BufReader<UnixStream>, Response)> {
        check_socket_path(path)?;
        let mut stream = UnixStream::connect(path)?;
        check_peer(&stream)?;
        wire::write(&mut stream, request)?;
        let mut reader = BufReader::new(stream);
        let reply = Self::read_response(&mut reader)?;
        Ok((reader, reply))
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
        let (reader, reply) = Self::open(
            path,
            Request::Join {
                token: token.into(),
            },
        )?;
        let Response::Joined(joined) = reply else {
            return Err(wire::invalid("expected a session join response"));
        };
        let client = Self {
            reader,
            member_id: Some(joined.member_id),
        };
        Ok((client, joined))
    }

    /// Request a permit on a separate, connection-scoped grant.
    pub(super) fn borrow_permit(path: &Path, token: &str) -> io::Result<BorrowOutcome> {
        let (reader, reply) = Self::open(
            path,
            Request::BorrowPermit {
                token: token.into(),
            },
        )?;
        match reply {
            Response::PermitGranted(permit) => Ok(BorrowOutcome::Granted {
                client: Self {
                    reader,
                    member_id: None,
                },
                permit,
            }),
            Response::PermitDenied { reason } => Ok(BorrowOutcome::Denied { reason }),
            _ => Err(wire::invalid("expected a session permit response")),
        }
    }

    /// Release a member with acknowledgement, or close a borrowed permit socket.
    pub(super) fn release(mut self) -> io::Result<()> {
        if let Some(member_id) = self.member_id {
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
    pub(super) fn wait_for_end(mut self) -> io::Result<()> {
        if !self.reader.fill_buf()?.is_empty() {
            return Err(wire::invalid("unexpected session broker message"));
        }
        Ok(())
    }
}
