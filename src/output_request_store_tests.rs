use super::*;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;

const ID: &str = "0123456789abcdef0123456789abcdef";

struct Fixture {
    root: PathBuf,
    outputs: File,
}

impl Fixture {
    fn new() -> Self {
        let output = Command::new("mktemp")
            .arg("-d")
            .arg(std::env::temp_dir().join("oxide-requests.XXXXXX"))
            .output()
            .expect("mktemp fixture");
        assert!(output.status.success());
        let root = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim_end());
        let outputs = File::open(&root).unwrap();
        Self { root, outputs }
    }

    fn store(&self) -> RequestStore {
        RequestStore::open(self.outputs.as_fd(), true).unwrap()
    }

    fn directory(&self) -> PathBuf {
        self.root.join(DIRECTORY)
    }

    fn seed(&self, name: &str, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.directory().join(name))
            .unwrap();
        file.write_all(bytes).unwrap();
    }
}

#[test]
fn read_never_initializes_and_publication_roundtrips_exact_bytes() {
    let fixture = Fixture::new();
    assert!(RequestStore::open(fixture.outputs.as_fd(), false).is_err());
    assert!(!fixture.directory().exists());
    let store = fixture.store();
    let bytes = br#"{"immutable":"original request"}"#;
    let sha = store.publish(ID, bytes).unwrap();
    assert_eq!(sha, digest(bytes));
    assert_eq!(store.read(ID, &sha).unwrap(), bytes);
    let path = fixture.directory().join(format!("{ID}.json"));
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!fixture.directory().join(format!(".{ID}.partial")).exists());
}

#[test]
fn a_published_or_partial_name_is_never_replaced_or_removed() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let sha = store.publish(ID, b"first").unwrap();
    assert!(store.publish(ID, b"second").is_err());
    assert_eq!(store.read(ID, &sha).unwrap(), b"first");
    let partial = fixture.directory().join(format!(".{ID}.partial"));
    assert_eq!(fs::read(&partial).unwrap(), b"second");
    assert!(store.publish(ID, b"third").is_err());
    assert_eq!(fs::read(partial).unwrap(), b"second");
}

#[test]
fn missing_changed_oversized_and_swapped_bytes_are_errors_not_absence() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let sha = digest(b"recorded");
    assert!(store.read(ID, &sha).is_err());
    let final_name = format!("{ID}.json");
    fixture.seed(&final_name, b"other");
    assert!(store.read(ID, &sha).is_err());
    fs::write(
        fixture.directory().join(&final_name),
        vec![0; MAX_REQUEST_BYTES + 1],
    )
    .unwrap();
    assert!(store.read(ID, &sha).is_err());
    assert!(store
        .publish(&"a".repeat(32), &vec![0; MAX_REQUEST_BYTES + 1])
        .is_err());
    assert!(store.publish(&"a".repeat(32), b"").is_err());
}

#[test]
fn links_nonprivate_storage_and_hardlinks_refuse() {
    let fixture = Fixture::new();
    let store = fixture.store();
    fixture.seed("foreign", b"foreign");
    let final_path = fixture.directory().join(format!("{ID}.json"));
    symlink("foreign", &final_path).unwrap();
    assert!(store.read(ID, &digest(b"foreign")).is_err());
    assert!(store.publish(&"a".repeat(32), b"new").is_err());
    assert_eq!(
        fs::read(fixture.directory().join("foreign")).unwrap(),
        b"foreign"
    );

    let hardlinked = Fixture::new();
    let hard_store = hardlinked.store();
    hardlinked.seed("source", b"original");
    fs::hard_link(
        hardlinked.directory().join("source"),
        hardlinked.directory().join(format!("{ID}.json")),
    )
    .unwrap();
    assert!(hard_store.read(ID, &digest(b"original")).is_err());

    fs::set_permissions(fixture.directory(), fs::Permissions::from_mode(0o750)).unwrap();
    assert!(store.read(ID, &digest(b"foreign")).is_err());
    assert!(RequestStore::open(fixture.outputs.as_fd(), true).is_err());
}

#[test]
fn every_retained_file_counts_and_unknown_entries_block_publication() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for index in 0..MAX_REQUEST_FILES {
        fixture.seed(&format!("partial-{index}"), b"kept");
    }
    assert!(store.publish(ID, b"new").is_err());
    assert_eq!(
        fs::read_dir(fixture.directory()).unwrap().count(),
        MAX_REQUEST_FILES
    );
    assert!(!fixture.directory().join(format!(".{ID}.partial")).exists());

    let unknown = Fixture::new();
    let unknown_store = unknown.store();
    fs::create_dir(unknown.directory().join("unexpected-directory")).unwrap();
    assert!(unknown_store.publish(ID, b"new").is_err());
}

#[test]
fn invalid_names_cannot_traverse_and_a_directory_link_is_not_followed() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for id in [
        "../outside",
        "ABCDEF0123456789abcdef0123456789ab",
        "",
        "a\0b",
    ] {
        assert!(store.publish(id, b"data").is_err());
        assert!(store.read(id, &digest(b"data")).is_err());
    }
    assert_eq!(fs::read_dir(fixture.directory()).unwrap().count(), 0);
    let linked = Fixture::new();
    fs::create_dir(linked.root.join("other")).unwrap();
    symlink("other", linked.directory()).unwrap();
    assert!(RequestStore::open(linked.outputs.as_fd(), true).is_err());
    assert_eq!(fs::read_dir(linked.root.join("other")).unwrap().count(), 0);
}

#[test]
fn caller_read_budget_failures_and_partial_reads_never_validate() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let sha = store.publish(ID, b"complete original request").unwrap();
    let denied = store.read_with(ID, &sha, |_, _| {
        Err(io::Error::other("authority budget exhausted"))
    });
    assert!(denied
        .unwrap_err()
        .to_string()
        .contains("authority budget exhausted"));
    assert!(store
        .read_with(ID, &sha, |mut reader, bytes| {
            let mut prefix = [0; 4];
            reader.read_exact(&mut prefix)?;
            bytes.extend_from_slice(&prefix);
            Ok(())
        })
        .is_err());
    assert_eq!(store.read(ID, &sha).unwrap(), b"complete original request");
}

#[test]
fn a_matching_prefix_digest_is_not_the_complete_file_image() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.publish(ID, b"prefix plus unconsumed tail").unwrap();
    assert!(store
        .read_with(ID, &digest(b"prefix"), |mut reader, bytes| {
            let mut prefix = [0; 6];
            reader.read_exact(&mut prefix)?;
            bytes.extend_from_slice(&prefix);
            Ok(())
        })
        .is_err());
}
