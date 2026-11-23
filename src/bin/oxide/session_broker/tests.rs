use super::*;

fn scratch_root() -> PathBuf {
    std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .expect("broker tests require a task scratch root")
        .into()
}

fn bound(mode: PermitMode, permits: impl PermitController) -> SessionBroker {
    SessionBroker::bind(&scratch_root(), mode, permits).unwrap()
}

fn broker(deadline: Option<Instant>) -> SessionBroker {
    let mut broker = bound(PermitMode::Inherited, DenyBorrowing);
    broker.arm(deadline).unwrap();
    broker
}

fn join(broker: &SessionBroker) -> (SessionClient, Joined) {
    SessionClient::join(&broker.directory.socket(), &broker.session.token).unwrap()
}

#[test]
fn join_and_release_use_the_advertised_endpoint() {
    let broker = broker(None);
    let mut command = Command::new("oxide");
    broker.apply(&mut command);
    let inherited: std::collections::HashMap<_, _> = command.get_envs().collect();
    let path = Path::new(inherited[OsStr::new(BROKER_ENV)].unwrap());
    let token = inherited[OsStr::new(TOKEN_ENV)].unwrap().to_str().unwrap();
    assert_eq!(token.len(), TOKEN_HEX_BYTES);
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let (client, joined) = SessionClient::join(path, token).unwrap();
    assert_eq!(
        joined,
        Joined {
            session_id: broker.session.id.clone(),
            member_id: 1,
            remaining_execution_ms: None,
            permit_mode: PermitMode::Inherited,
        }
    );
    assert_eq!(broker.member_count(), 1);
    client.release().unwrap();
    assert_eq!(broker.member_count(), 0);
}

#[test]
fn handshakes_wait_for_the_fixed_execution_deadline() {
    use std::io::Write;

    let mut broker = bound(PermitMode::Inherited, DenyBorrowing);
    let mut socket = UnixStream::connect(broker.directory.socket()).unwrap();
    writeln!(
        socket,
        r#"{{"version":1,"body":{{"type":"join","token":"{}"}}}}"#,
        broker.session.token
    )
    .unwrap();
    socket.set_nonblocking(true).unwrap();
    assert_eq!(
        socket.read(&mut [0u8]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    broker.arm(Some(deadline)).unwrap();
    assert_eq!(
        broker.arm(None).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    let mut reply = String::new();
    BufReader::new(socket).read_line(&mut reply).unwrap();
    let frame: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(frame["version"], 1);
    assert_eq!(frame["body"]["type"], "joined");
    let first_ms = frame["body"]["remaining_execution_ms"].as_u64().unwrap();
    assert!(first_ms <= 30_000);
    thread::sleep(Duration::from_millis(30));
    let (_client, joined) = join(&broker);
    assert!(joined.remaining_execution_ms.unwrap() < first_ms);

    let expired = self::broker(Some(Instant::now()));
    let (_client, joined) = join(&expired);
    assert_eq!(joined.remaining_execution_ms, Some(0));
}

fn assert_refused(broker: &SessionBroker, bytes: &[u8]) {
    use std::io::Write;

    let mut socket = UnixStream::connect(broker.directory.socket()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    if let Err(error) = socket.write_all(bytes) {
        assert!(matches!(
            error.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
        ));
    }
    match socket.read(&mut [0u8]) {
        Ok(0) => {}
        Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
        result => panic!("invalid frame did not close without a response: {result:?}"),
    }
    assert_eq!(broker.member_count(), 0);
}

#[test]
fn rejects_an_incorrect_token_of_the_right_length() {
    let broker = broker(None);
    let mut wrong_token = broker.session.token.clone().into_bytes();
    wrong_token[0] = if wrong_token[0] == b'0' { b'1' } else { b'0' };
    let wrong_token = String::from_utf8(wrong_token).unwrap();
    let error = SessionClient::join(&broker.directory.socket(), &wrong_token).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert_eq!(broker.member_count(), 0);
}

#[test]
fn rejects_oversized_malformed_and_unknown_version_frames() {
    let broker = broker(None);
    let join = format!(
        r#"{{"version":1,"body":{{"type":"join","token":"{}"}}}}"#,
        broker.session.token
    );

    let mut oversized = join.clone().into_bytes();
    oversized.resize(wire::MAX_FRAME_BYTES, b' ');
    oversized.push(b'\n');
    assert_refused(&broker, &oversized);
    assert_refused(&broker, b"{broken\n");
    assert_refused(
        &broker,
        format!("{}\n", join.replace("\"version\":1", "\"version\":2")).as_bytes(),
    );
    assert_refused(
        &broker,
        format!("{},\"extra\":0}}\n", join.strip_suffix('}').unwrap()).as_bytes(),
    );

    let (client, _) = self::join(&broker);
    client.release().unwrap();
}

#[test]
fn disconnect_cleans_up_a_joined_member() {
    let broker = broker(None);
    let (client, _) = join(&broker);
    assert_eq!(broker.member_count(), 1);
    drop(client);
    let until = Instant::now() + Duration::from_secs(2);
    while broker.member_count() != 0 {
        assert!(Instant::now() < until, "disconnected member stayed active");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn drop_closes_live_sockets_and_removes_only_the_private_directory() {
    let broker = broker(None);
    let directory = broker.directory.path.clone();
    let socket = broker.directory.socket();
    let (client, _) = join(&broker);
    client
        .reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut pending = UnixStream::connect(&socket).unwrap();
    pending
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    drop(broker);
    client.wait_for_end().unwrap();
    match pending.read(&mut [0]) {
        Ok(0) => {}
        Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
        result => panic!("pending connection survived shutdown: {result:?}"),
    }
    assert!(!socket.exists());
    assert!(!directory.exists());
    assert!(directory.parent().unwrap().is_dir());
}

#[test]
fn socket_directory_is_private_and_unarmed_drop_cleans_it() {
    let broker = bound(PermitMode::Inherited, DenyBorrowing);
    let directory = broker.directory.path.clone();
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(broker.directory.socket().exists());
    drop(broker);
    assert!(!directory.exists());
}

#[test]
fn refuses_an_overlong_socket_path_before_creating_a_directory() {
    let fixture = SocketDirectory::create(&scratch_root()).unwrap();
    let root = fixture.path.join("long-root".repeat(15));
    fs::create_dir(&root).unwrap();
    let result = SessionBroker::bind(&root, PermitMode::Inherited, DenyBorrowing);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::remove_dir(root).unwrap();
    assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn refuses_a_private_root_beneath_a_writable_nonsticky_ancestor() {
    let fixture = SocketDirectory::create(&scratch_root()).unwrap();
    let shared = fixture.path.join("shared");
    let root = shared.join("temp");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&shared, Permissions::from_mode(0o770)).unwrap();

    let result = SessionBroker::bind(&root, PermitMode::Inherited, DenyBorrowing);
    let error = result
        .err()
        .expect("the writable ancestor should refuse binding");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains(&shared.display().to_string()));
    assert!(fs::read_dir(&root).unwrap().next().is_none());

    // A sticky parent protects entries owned by the trusted user.
    fs::set_permissions(&shared, Permissions::from_mode(0o1770)).unwrap();
    let broker = SessionBroker::bind(&root, PermitMode::Inherited, DenyBorrowing).unwrap();
    drop(broker);
    fs::remove_dir(root).unwrap();
    fs::remove_dir(shared).unwrap();
}

#[test]
fn socket_path_accepts_the_platform_limit_and_refuses_one_more_byte() {
    let accepted = format!("/{}", "a".repeat(MAX_SOCKET_PATH_BYTES - 1));
    assert_eq!(accepted.len(), MAX_SOCKET_PATH_BYTES);
    assert!(check_socket_path(Path::new(&accepted)).is_ok());
    let refused = format!("{accepted}a");
    assert_eq!(
        check_socket_path(Path::new(&refused)).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    #[cfg(target_os = "macos")]
    assert_eq!(SUN_PATH_BYTES, 104);
    #[cfg(target_os = "linux")]
    assert_eq!(SUN_PATH_BYTES, 108);
}

#[cfg(target_os = "macos")]
#[test]
fn refuses_an_ancestor_with_an_acl_grant() {
    let fixture = SocketDirectory::create(&scratch_root()).unwrap();
    let root = fixture.path.join("temp");
    fs::create_dir(&root).unwrap();
    let output = Command::new("/bin/chmod")
        .args(["+a", "everyone allow delete_child"])
        .arg(&fixture.path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::metadata(&fixture.path).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let result = SessionBroker::bind(&root, PermitMode::Inherited, DenyBorrowing);
    let error = result.err().expect("the ACL grant should refuse binding");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("ACL grants access"));
    assert!(fs::read_dir(&root).unwrap().next().is_none());
    fs::remove_dir(root).unwrap();
}

/// A named-user entry makes the Linux POSIX ACL extended, so it grants access.
#[cfg(target_os = "linux")]
#[test]
fn refuses_an_ancestor_with_an_acl_grant() {
    const ACL_VERSION: u32 = 2;
    const USER_OBJ: u16 = 0x01;
    const NAMED_USER: u16 = 0x02;
    const GROUP_OBJ: u16 = 0x04;
    const MASK: u16 = 0x10;
    const OTHER: u16 = 0x20;
    const NOBODY: u32 = 65534;
    const UNDEFINED_ID: u32 = u32::MAX;

    let fixture = SocketDirectory::create(&scratch_root()).unwrap();
    let root = fixture.path.join("temp");
    fs::create_dir(&root).unwrap();
    let entries: [(u16, u16, u32); 5] = [
        (USER_OBJ, 7, UNDEFINED_ID),
        (NAMED_USER, 7, NOBODY),
        (GROUP_OBJ, 0, UNDEFINED_ID),
        (MASK, 7, UNDEFINED_ID),
        (OTHER, 0, UNDEFINED_ID),
    ];
    let mut blob = ACL_VERSION.to_le_bytes().to_vec();
    for (tag, perm, id) in entries {
        blob.extend_from_slice(&tag.to_le_bytes());
        blob.extend_from_slice(&perm.to_le_bytes());
        blob.extend_from_slice(&id.to_le_bytes());
    }
    let path = CString::new(fixture.path.as_os_str().as_bytes()).unwrap();
    let attribute = c"system.posix_acl_access";
    // SAFETY: both names are NUL-terminated and `blob` is valid for its length.
    let status = unsafe {
        libc::lsetxattr(
            path.as_ptr(),
            attribute.as_ptr(),
            blob.as_ptr().cast(),
            blob.len(),
            0,
        )
    };
    assert_eq!(status, 0, "{}", io::Error::last_os_error());

    let result = SessionBroker::bind(&root, PermitMode::Inherited, DenyBorrowing);
    let error = result.err().expect("the ACL grant should refuse binding");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("ACL grants access"));
    assert!(fs::read_dir(&root).unwrap().next().is_none());
    fs::remove_dir(root).unwrap();
}

struct OnePermit {
    borrowed: Arc<std::sync::Mutex<Option<u64>>>,
}

struct TestGrant {
    borrowed: Arc<std::sync::Mutex<Option<u64>>>,
    id: u64,
}

impl Drop for TestGrant {
    fn drop(&mut self) {
        assert_eq!(self.borrowed.lock().unwrap().take(), Some(self.id));
    }
}

impl PermitController for OnePermit {
    fn borrow(&self, id: u64, _: &BorrowWait<'_>) -> Result<PermitGrant, PermitDenial> {
        let mut borrowed = self.borrowed.lock().unwrap();
        if borrowed.is_some() {
            return Err(PermitDenial::Busy);
        }
        *borrowed = Some(id);
        Ok(Box::new(TestGrant {
            borrowed: Arc::clone(&self.borrowed),
            id,
        }))
    }
}

#[test]
fn permit_policy_controls_grant_and_broker_drop_returns_it() {
    let borrowed = Arc::new(std::sync::Mutex::new(None));
    let mut broker = bound(
        PermitMode::BorrowRequired,
        OnePermit {
            borrowed: Arc::clone(&borrowed),
        },
    );
    broker
        .arm(Some(Instant::now() + Duration::from_secs(30)))
        .unwrap();
    let path = broker.directory.socket();
    let token = &broker.session.token;
    let (_member, joined) = join(&broker);
    assert_eq!(joined.permit_mode, PermitMode::BorrowRequired);

    let BorrowOutcome::Granted { client, permit } =
        SessionClient::borrow_permit(&path, token, None, || Ok(())).unwrap()
    else {
        panic!("first borrower should get the available permit");
    };
    assert_eq!(*borrowed.lock().unwrap(), Some(permit.borrow_id));
    assert!(permit.remaining_execution_ms.unwrap() <= joined.remaining_execution_ms.unwrap());
    assert!(matches!(
        SessionClient::borrow_permit(&path, token, None, || Ok(())).unwrap(),
        BorrowOutcome::Denied {
            reason: PermitDenial::Busy
        }
    ));

    client.release().unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while borrowed.lock().unwrap().is_some() {
        assert!(
            Instant::now() < until,
            "disconnected borrower kept its permit"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let BorrowOutcome::Granted { client, .. } =
        SessionClient::borrow_permit(&path, token, None, || Ok(())).unwrap()
    else {
        panic!("first borrower should have returned the permit");
    };
    drop(broker);
    client.wait_for_end().unwrap();
    assert_eq!(*borrowed.lock().unwrap(), None);
}
