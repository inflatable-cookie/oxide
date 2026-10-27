//! Pure and tiny-fixture regressions for the slice-1 source verification
//! (oxide#166).
//!
//! Archive fixtures are built in memory; extracted trees, caches and git
//! checkouts are tiny directories under the ordinary test temp directory
//! (`TMPDIR`, on the selected volume), never inside the proof supervisor's
//! monitored root. Every fixture is owned and removed by its guard. No Cargo,
//! pool, lease, install or removable volume is involved.

use super::archive::derive;
use super::cache::{CacheRecord, SourceCache, MAX_CACHE_BYTES, MAX_CACHE_RECORDS};
use super::digest::{DigestScheme, SourceTree, TreeDigest, MAX_PATH_BYTES};
use super::git::{
    clears, git_command, GitRefusal, GitVerifier, GIT_CALL_TIME, GIT_SETTLE_TIME, GIT_TOTAL_TIME,
    MAX_GIT_CALLS, MAX_GIT_STDOUT_BYTES,
};
use super::{Refusal, RegistrySource, SourceLimits, VerifiedRegistrySource};
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::fs::FileTimes;
use std::io::{Cursor, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tar::{Builder, EntryType, Header};

const INDEX: &str = "index.crates.io-1949cf8c6b5b557f";
const TARBALL: &str = "cfg-if-1.0.0";
static NEXT: AtomicU64 = AtomicU64::new(1);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir().join("oxide-seed-source");
        fs::create_dir_all(&base).expect("fixture parent");
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("{label}-{:x}-{id:x}", std::process::id()));
        fs::create_dir(&path).expect("fixture directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_file(path: &Path, data: &[u8], mode: u32) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, data).expect("fixture file");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

fn set_mtime(path: &Path, modified: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime");
    file.set_times(FileTimes::new().set_modified(modified))
        .expect("set mtime");
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

enum Spec<'a> {
    File { data: &'a [u8], mode: u32 },
    Dir,
    Symlink(&'a str),
    Hardlink(&'a str),
    Char,
    Fifo,
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

fn build_archive(entries: &[(String, Spec<'_>)]) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    for (path, spec) in entries {
        match spec {
            Spec::File { data, mode } => {
                let mut header = Header::new_gnu();
                header.set_mode(*mode);
                header.set_size(u64::try_from(data.len()).expect("fixture size"));
                builder
                    .append_data(&mut header, path, *data)
                    .expect("append file");
            }
            Spec::Dir => {
                let mut header = Header::new_gnu();
                header.set_mode(0o755);
                header.set_entry_type(EntryType::Directory);
                header.set_size(0);
                builder
                    .append_data(&mut header, path, &[][..])
                    .expect("append directory");
            }
            Spec::Symlink(target) => {
                let mut header = Header::new_gnu();
                header.set_mode(0o777);
                header.set_entry_type(EntryType::Symlink);
                header.set_size(0);
                header.set_link_name(target).expect("link name");
                header.set_cksum();
                builder.append(&header, &[][..]).expect("append symlink");
            }
            Spec::Hardlink(target) => {
                let mut header = Header::new_gnu();
                header.set_mode(0o644);
                header.set_entry_type(EntryType::Link);
                header.set_size(0);
                header.set_link_name(target).expect("link name");
                header.set_cksum();
                builder.append(&header, &[][..]).expect("append hardlink");
            }
            Spec::Char => {
                let mut header = Header::new_gnu();
                header.set_mode(0o644);
                header.set_entry_type(EntryType::Char);
                header.set_size(0);
                header.set_cksum();
                builder.append(&header, &[][..]).expect("append device");
            }
            Spec::Fifo => {
                let mut header = Header::new_gnu();
                header.set_mode(0o644);
                header.set_entry_type(EntryType::Fifo);
                header.set_size(0);
                header.set_cksum();
                builder.append(&header, &[][..]).expect("append fifo");
            }
        }
    }
    let tar = builder.into_inner().expect("finish tar");
    gzip(&tar)
}

/// One raw ustar block, so a fixture can carry a path the writer refuses to
/// construct (absolute, `..`, non-UTF-8, or a pax description).
fn raw_block(name: &[u8], type_flag: u8, data: &[u8]) -> Vec<u8> {
    raw_block_sized(
        name,
        type_flag,
        u64::try_from(data.len()).expect("fixture size"),
        data,
    )
}

/// One raw ustar block whose declared size is independent of the bytes that
/// follow, so a fixture can present an extension header that claims far more
/// payload than it carries.
fn raw_block_sized(name: &[u8], type_flag: u8, declared: u64, data: &[u8]) -> Vec<u8> {
    assert!(name.len() <= 100, "fixture header name");
    let mut header = [0_u8; 512];
    header[..name.len()].copy_from_slice(name);
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..136].copy_from_slice(format!("{declared:011o}\0").as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].copy_from_slice(b"        ");
    header[156] = type_flag;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut block = header.to_vec();
    block.extend_from_slice(data);
    block.extend(std::iter::repeat_n(0_u8, (512 - data.len() % 512) % 512));
    block
}

fn pax_record(key: &str, value: &str) -> Vec<u8> {
    let body = format!(" {key}={value}\n");
    let mut length = body.len() + 1;
    loop {
        let record = format!("{length}{body}");
        if record.len() == length {
            return record.into_bytes();
        }
        length = record.len();
    }
}

fn pax_archive(path: &str, data: &[u8]) -> Vec<u8> {
    let record = pax_record("path", path);
    let mut tar = raw_block(b"PaxHeaders/x", b'x', &record);
    tar.extend(raw_block(b"placeholder", b'0', data));
    tar.extend([0_u8; 1024]);
    gzip(&tar)
}

/// One registry package fixture: `$CARGO_HOME/registry/{cache,src}/<index>`.
struct PackageFixture {
    _temp: TempDir,
    cargo_home: PathBuf,
    tarball: String,
    archive: PathBuf,
    tree: PathBuf,
    checksum: String,
}

impl PackageFixture {
    fn create(label: &str, files: &[(&str, &[u8], u32)]) -> Self {
        let entries: Vec<(String, Spec<'_>)> = files
            .iter()
            .map(|(path, data, mode)| {
                (
                    format!("{TARBALL}/{path}"),
                    Spec::File { data, mode: *mode },
                )
            })
            .collect();
        Self::from_archive(label, &build_archive(&entries), files)
    }

    fn from_archive(label: &str, archive: &[u8], files: &[(&str, &[u8], u32)]) -> Self {
        let temp = TempDir::new(label);
        let cargo_home = temp.path().join("cargo-home");
        let archive_path = cargo_home
            .join("registry")
            .join("cache")
            .join(INDEX)
            .join(format!("{TARBALL}.crate"));
        write_file(&archive_path, archive, 0o644);
        let tree = cargo_home
            .join("registry")
            .join("src")
            .join(INDEX)
            .join(TARBALL);
        for (path, data, mode) in files {
            write_file(&tree.join(path), data, *mode);
        }
        write_file(&tree.join(".cargo-ok"), b"", 0o644);
        Self {
            _temp: temp,
            cargo_home,
            tarball: TARBALL.to_owned(),
            archive: archive_path,
            tree,
            checksum: sha256_hex(archive),
        }
    }

    fn locate(&self) -> RegistrySource {
        RegistrySource::locate(&self.cargo_home, &self.tarball).expect("locate fixture")
    }

    fn cache(&self) -> SourceCache {
        SourceCache::new("store-fixture").expect("cache")
    }

    fn verify(&self, cache: &mut SourceCache) -> Result<VerifiedRegistrySource, Refusal> {
        let source = match RegistrySource::locate(&self.cargo_home, &self.tarball) {
            Ok(source) => source,
            Err(refusal) => return Err(refusal),
        };
        source.verify(&self.checksum, &SourceLimits::default(), cache, 1_000)
    }
}

fn simple_files() -> Vec<(&'static str, &'static [u8], u32)> {
    vec![
        (
            "Cargo.toml",
            b"[package]\nname = \"cfg-if\"\n".as_slice(),
            0o644,
        ),
        ("src/lib.rs", b"pub fn cfg() {}\n".as_slice(), 0o644),
        ("src/nested/mod.rs", b"mod nested;\n".as_slice(), 0o644),
        ("build.sh", b"#!/bin/sh\n".as_slice(), 0o755),
    ]
}

fn archive_error(bytes: Vec<u8>, tarball: &str) -> Option<Refusal> {
    derive(
        Cursor::new(bytes),
        tarball,
        &SourceLimits::default(),
        Instant::now(),
    )
    .err()
}

#[test]
fn a_registry_package_derives_its_tree_and_hits_the_cache_without_rereading_the_archive() {
    let fixture = PackageFixture::create("positive", &simple_files());
    let mut cache = fixture.cache();
    let first = fixture.verify(&mut cache).expect("first verification");
    assert!(!first.cache_hit);
    assert_eq!(first.digest.to_hex().len(), 64);
    assert_eq!(first.entries, 6, "four files and two directories");
    assert!(first.content_bytes > 0);
    assert!(first.derived_bytes > 0);
    assert_eq!(cache.len(), 1);
    assert_eq!(first.archive_sha256, fixture.checksum);

    let second = fixture.verify(&mut cache).expect("second verification");
    assert!(second.cache_hit);
    assert_eq!(second.digest, first.digest);
    assert_eq!(second.content_bytes, first.content_bytes);

    // §4.3 lists an unreadable archive among the identity changes that force a
    // full re-derivation or a skip. A hit opens the archive (no follow) to
    // establish readability and identity, reading no content byte, so an
    // archive that loses read access refuses instead of hitting.
    fs::set_permissions(&fixture.archive, fs::Permissions::from_mode(0o000))
        .expect("make the archive unreadable");
    assert_eq!(
        fixture.verify(&mut cache).err(),
        Some(Refusal::ArchiveUnreadable)
    );
    fs::set_permissions(&fixture.archive, fs::Permissions::from_mode(0o644))
        .expect("restore archive mode");
    let third = fixture.verify(&mut cache).expect("hit once readable again");
    assert!(third.cache_hit);
    assert_eq!(third.digest, first.digest);
}

#[test]
fn a_cache_hit_binds_the_archive_identity_and_does_not_read_its_bytes() {
    // The design's stated cooperative residual: a same-user rewrite that
    // preserves the archive's device, inode, size and mtime is not detected by
    // a hit, because a hit deliberately reads no archive content. Overwriting
    // the bytes in place with the same length and restoring the mtime proves
    // the hit path never consulted them; a fresh cache on the same bytes does
    // re-derive and refuses.
    let fixture = PackageFixture::create("cache-identity", &simple_files());
    let mut cache = fixture.cache();
    let first = fixture.verify(&mut cache).expect("first verification");
    let metadata = fs::metadata(&fixture.archive).expect("archive metadata");
    let modified = metadata.modified().expect("archive mtime");
    let size = metadata.len();
    fs::write(
        &fixture.archive,
        vec![0_u8; usize::try_from(size).expect("archive size")],
    )
    .expect("rewrite the archive in place");
    set_mtime(&fixture.archive, modified);
    let hit = fixture.verify(&mut cache).expect("identity-bound hit");
    assert!(hit.cache_hit);
    assert_eq!(hit.digest, first.digest);
    assert!(
        fixture.verify(&mut fixture.cache()).is_err(),
        "a fresh derivation sees the changed bytes"
    );
}

#[test]
fn the_cache_misses_when_the_archive_identity_or_the_lock_checksum_changes() {
    let fixture = PackageFixture::create("identity", &simple_files());
    let mut cache = fixture.cache();
    fixture.verify(&mut cache).expect("first verification");

    // Same bytes, a new inode.
    let bytes = fs::read(&fixture.archive).expect("archive bytes");
    let replacement = fixture.archive.with_file_name("replacement.crate");
    write_file(&replacement, &bytes, 0o644);
    fs::rename(&replacement, &fixture.archive).expect("replace archive");
    let by_inode = fixture
        .verify(&mut cache)
        .expect("re-derived after inode change");
    assert!(!by_inode.cache_hit);

    // Same bytes and inode, a new mtime.
    set_mtime(
        &fixture.archive,
        SystemTime::now() + Duration::from_secs(120),
    );
    let by_mtime = fixture
        .verify(&mut cache)
        .expect("re-derived after mtime change");
    assert!(!by_mtime.cache_hit);

    // A forged device field is not a hit.
    let mut record = cache.records()[0].clone();
    record.archive_device = record.archive_device.wrapping_add(1);
    cache.insert(record).expect("forge device");
    let by_device = fixture
        .verify(&mut cache)
        .expect("re-derived after device change");
    assert!(!by_device.cache_hit);

    // A different lock checksum can never use the record and refuses the
    // archive bytes, whose SHA-256 is the original checksum.
    let mut other = fixture.cache();
    other
        .insert(cache.records()[0].clone())
        .expect("seed cache record");
    let refused =
        fixture
            .locate()
            .verify(&"0".repeat(64), &SourceLimits::default(), &mut other, 1_000);
    assert_eq!(refused.err(), Some(Refusal::ChecksumMismatch));

    // An untrackable archive identity (a symlink) refuses instead of hitting.
    let target = fixture.archive.with_file_name("target.crate");
    write_file(&target, &bytes, 0o644);
    fs::remove_file(&fixture.archive).expect("remove archive");
    symlink(&target, &fixture.archive).expect("link archive");
    assert_eq!(
        fixture.verify(&mut cache).err(),
        Some(Refusal::ArchiveUnreadable)
    );
}

#[test]
fn the_extracted_tree_must_equal_the_archive_exactly() {
    for (label, expected) in [
        ("extra", Refusal::TreeMismatch("extra")),
        ("missing", Refusal::TreeMismatch("missing")),
        ("content", Refusal::TreeMismatch("content")),
        ("size", Refusal::TreeMismatch("size")),
        ("executable", Refusal::TreeMismatch("executable")),
    ] {
        let fixture = PackageFixture::create(label, &simple_files());
        match label {
            "extra" => write_file(&fixture.tree.join("src/extra.rs"), b"extra\n", 0o644),
            "missing" => {
                fs::remove_file(fixture.tree.join("src/nested/mod.rs")).expect("remove");
                fs::remove_dir(fixture.tree.join("src/nested")).expect("remove dir");
            }
            "content" => write_file(
                &fixture.tree.join("src/lib.rs"),
                b"pub fn cfh() {}\n",
                0o644,
            ),
            "size" => write_file(
                &fixture.tree.join("src/lib.rs"),
                b"pub fn cfg() {}\n\n",
                0o644,
            ),
            "executable" => fs::set_permissions(
                fixture.tree.join("src/lib.rs"),
                fs::Permissions::from_mode(0o755),
            )
            .expect("chmod"),
            _ => unreachable!("fixture label"),
        }
        let mut cache = fixture.cache();
        assert_eq!(fixture.verify(&mut cache).err(), Some(expected), "{label}");
        assert!(cache.is_empty(), "{label} records nothing");
    }
}

#[test]
fn cargo_ok_is_never_evidence() {
    let fixture = PackageFixture::create("cargo-ok", &simple_files());
    let mut cache = fixture.cache();
    fixture.verify(&mut cache).expect("a marker is ignored");
    // A marker directory or symlink is not the marker and stays a tree entry.
    fs::remove_file(fixture.tree.join(".cargo-ok")).expect("remove marker");
    fs::create_dir(fixture.tree.join(".cargo-ok")).expect("marker directory");
    assert_eq!(
        fixture.verify(&mut cache).err(),
        Some(Refusal::TreeMismatch("extra"))
    );
    fs::remove_dir(fixture.tree.join(".cargo-ok")).expect("remove marker directory");
    symlink("Cargo.toml", fixture.tree.join(".cargo-ok")).expect("marker symlink");
    assert_eq!(
        fixture.verify(&mut cache).err(),
        Some(Refusal::TreeGrammar("symlink"))
    );
}

#[test]
fn the_archive_entry_grammar_is_strict() {
    // Explicit directory entries are accepted; crates.io usually omits them.
    let with_directory = build_archive(&[
        (format!("{TARBALL}/src"), Spec::Dir),
        (
            format!("{TARBALL}/src/lib.rs"),
            Spec::File {
                data: b"x",
                mode: 0o644,
            },
        ),
    ]);
    let derived = derive(
        Cursor::new(with_directory),
        TARBALL,
        &SourceLimits::default(),
        Instant::now(),
    )
    .expect("a directory entry is accepted");
    assert!(derived.tree.get("src").is_some());
    let cases: Vec<(&str, Vec<u8>, Refusal)> = vec![
        (
            "symlink",
            build_archive(&[(format!("{TARBALL}/link"), Spec::Symlink("target"))]),
            Refusal::ArchiveGrammar("symlink"),
        ),
        (
            "hardlink",
            build_archive(&[(format!("{TARBALL}/link"), Spec::Hardlink("target"))]),
            Refusal::ArchiveGrammar("hardlink"),
        ),
        (
            "device",
            build_archive(&[(format!("{TARBALL}/dev"), Spec::Char)]),
            Refusal::ArchiveGrammar("device"),
        ),
        (
            "fifo",
            build_archive(&[(format!("{TARBALL}/fifo"), Spec::Fifo)]),
            Refusal::ArchiveGrammar("fifo"),
        ),
        (
            "duplicate",
            build_archive(&[
                (
                    format!("{TARBALL}/same"),
                    Spec::File {
                        data: b"x",
                        mode: 0o644,
                    },
                ),
                (
                    format!("{TARBALL}/same"),
                    Spec::File {
                        data: b"y",
                        mode: 0o644,
                    },
                ),
            ]),
            Refusal::ArchiveGrammar("duplicate"),
        ),
        (
            "absolute",
            gzip(&raw_block(b"/absolute/file", b'0', b"x")),
            Refusal::ArchiveGrammar("absolute"),
        ),
        (
            "parent",
            gzip(&raw_block(b"cfg-if-1.0.0/../evil", b'0', b"x")),
            Refusal::ArchiveGrammar("parent"),
        ),
        (
            "current",
            gzip(&raw_block(b"cfg-if-1.0.0/./evil", b'0', b"x")),
            Refusal::ArchiveGrammar("current"),
        ),
        (
            "non-utf8",
            gzip(&raw_block(b"cfg-if-1.0.0/\xff", b'0', b"x")),
            Refusal::ArchiveGrammar("non-utf8"),
        ),
        (
            "prefix",
            build_archive(&[(
                "other-1.0.0/file".to_owned(),
                Spec::File {
                    data: b"x",
                    mode: 0o644,
                },
            )]),
            Refusal::ArchiveGrammar("prefix"),
        ),
        (
            "too-long",
            build_archive(&[(
                format!("{TARBALL}/{}", "a".repeat(MAX_PATH_BYTES + 1)),
                Spec::File {
                    data: b"x",
                    mode: 0o644,
                },
            )]),
            Refusal::ArchiveGrammar("too-long"),
        ),
    ];
    for (label, bytes, expected) in cases {
        assert_eq!(archive_error(bytes, TARBALL), Some(expected), "{label}");
    }
}

#[test]
fn a_truncated_or_unparseable_archive_refuses() {
    let valid = build_archive(&[(
        format!("{TARBALL}/Cargo.toml"),
        Spec::File {
            data: b"[package]\n",
            mode: 0o644,
        },
    )]);
    let mut truncated = valid.clone();
    truncated.truncate(valid.len().saturating_sub(40));
    assert_eq!(
        archive_error(truncated, TARBALL),
        Some(Refusal::ArchiveCorrupt)
    );
    assert_eq!(
        archive_error(b"not a gzip stream".to_vec(), TARBALL),
        Some(Refusal::ArchiveCorrupt)
    );
    // A tampered archive cannot match the lock checksum even when it still
    // decodes.
    let fixture =
        PackageFixture::from_archive("tampered", &valid, &[("Cargo.toml", b"[package]\n", 0o644)]);
    let mut tampered = valid.clone();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0xff;
    write_file(&fixture.archive, &tampered, 0o644);
    let mut cache = fixture.cache();
    let refused = fixture.locate().verify(
        &fixture.checksum,
        &SourceLimits::default(),
        &mut cache,
        1_000,
    );
    assert!(
        matches!(
            refused,
            Err(Refusal::ArchiveCorrupt | Refusal::ChecksumMismatch)
        ),
        "{refused:?}"
    );
}

#[test]
fn gnu_and_pax_long_names_are_accepted_and_an_escaping_pax_path_refuses() {
    let long = format!("{TARBALL}/{}", "a".repeat(140));
    let gnu = build_archive(&[(
        long.clone(),
        Spec::File {
            data: b"long\n",
            mode: 0o644,
        },
    )]);
    let relative = long.trim_start_matches(&format!("{TARBALL}/")).to_owned();
    let fixture = PackageFixture::from_archive(
        "gnu-long",
        &gnu,
        &[(relative.as_str(), b"long\n".as_slice(), 0o644)],
    );
    let mut cache = fixture.cache();
    fixture
        .verify(&mut cache)
        .expect("a GNU long name is accepted");

    let pax = pax_archive(&format!("{TARBALL}/{relative}"), b"long\n");
    let fixture = PackageFixture::from_archive(
        "pax-long",
        &pax,
        &[(relative.as_str(), b"long\n".as_slice(), 0o644)],
    );
    let mut cache = fixture.cache();
    fixture
        .verify(&mut cache)
        .expect("a pax long name is accepted");

    let escaping = pax_archive(&format!("{TARBALL}/../evil"), b"x");
    assert_eq!(
        archive_error(escaping, TARBALL),
        Some(Refusal::ArchiveGrammar("parent"))
    );
}

#[test]
fn a_registry_source_is_located_only_through_one_unambiguous_index() {
    let fixture = PackageFixture::create("locate", &simple_files());
    let located = fixture.locate();
    assert_eq!(located.archive(), fixture.archive);
    assert_eq!(located.tree(), fixture.tree);
    assert_eq!(
        located.archive_relative(),
        format!("registry/cache/{INDEX}/{TARBALL}.crate")
    );

    // A missing cache directory is a missing archive, not an error.
    let empty = TempDir::new("locate-empty");
    assert_eq!(
        RegistrySource::locate(empty.path(), TARBALL).err(),
        Some(Refusal::MissingArchive)
    );
    // Two index directories holding the archive is ambiguous.
    let other = fixture
        .cargo_home
        .join("registry/cache/other-index")
        .join(format!("{TARBALL}.crate"));
    write_file(&other, b"second", 0o644);
    assert_eq!(
        RegistrySource::locate(&fixture.cargo_home, TARBALL).err(),
        Some(Refusal::AmbiguousRegistryIndex)
    );
    fs::remove_file(&other).expect("remove second archive");
    // A missing extracted tree refuses.
    let tree = fixture.tree.clone();
    fs::rename(&tree, fixture.tree.with_file_name("moved")).expect("move tree");
    assert_eq!(
        RegistrySource::locate(&fixture.cargo_home, TARBALL).err(),
        Some(Refusal::MissingTree)
    );
    let moved = fixture.tree.with_file_name("moved");
    symlink(&moved, &tree).expect("symlink tree");
    assert_eq!(
        RegistrySource::locate(&fixture.cargo_home, TARBALL).err(),
        Some(Refusal::TreeUnreadable)
    );
    assert_eq!(
        RegistrySource::locate(&fixture.cargo_home, "../escape").err(),
        Some(Refusal::Unsupported)
    );
}

#[test]
fn the_source_bounds_refuse_before_unbounded_work() {
    let fixture = PackageFixture::create("bounds", &simple_files());
    let mut cache = fixture.cache();
    let immediate = SourceLimits {
        deadline: Duration::ZERO,
        ..SourceLimits::default()
    };
    assert_eq!(
        fixture
            .locate()
            .verify(&fixture.checksum, &immediate, &mut cache, 1)
            .err(),
        Some(Refusal::Deadline)
    );
    let tiny_content = SourceLimits {
        max_content_bytes: 1,
        ..SourceLimits::default()
    };
    assert_eq!(
        fixture
            .locate()
            .verify(&fixture.checksum, &tiny_content, &mut cache, 1)
            .err(),
        Some(Refusal::TreeBudget)
    );
    let tiny_archive = SourceLimits {
        max_derived_bytes: 1,
        ..SourceLimits::default()
    };
    assert_eq!(
        fixture
            .locate()
            .verify(&fixture.checksum, &tiny_archive, &mut cache, 1)
            .err(),
        Some(Refusal::ArchiveBudget)
    );
    let tiny_entries = SourceLimits {
        max_entries: 1,
        ..SourceLimits::default()
    };
    assert_eq!(
        fixture
            .locate()
            .verify(&fixture.checksum, &tiny_entries, &mut cache, 1)
            .err(),
        Some(Refusal::ArchiveBudget)
    );
}

fn sample_digest() -> TreeDigest {
    SourceTree::new()
        .digest(DigestScheme::RegistrySource)
        .expect("empty digest")
}

fn sample_record(archive: &str, verified_at: u64) -> CacheRecord {
    CacheRecord {
        cargo_home: "/private/tmp/oxide-home".to_owned(),
        archive: archive.to_owned(),
        archive_device: 1,
        archive_inode: 2,
        archive_size: 3,
        archive_mtime_secs: 4,
        archive_mtime_nanos: 5,
        archive_sha256: "a".repeat(64),
        tree_device: 6,
        tree_inode: 7,
        tree_digest: sample_digest().to_hex(),
        verified_at,
    }
}

#[test]
fn the_cache_document_is_owner_only_bounded_and_ignored_when_invalid() {
    let temp = TempDir::new("cache-document");
    let path = temp.path().join("sources.json");
    let mut cache = SourceCache::new("store-1").expect("cache");
    cache
        .insert(sample_record("registry/cache/i/cfg-if-1.0.0.crate", 5))
        .expect("insert");
    cache.write(&path).expect("write");
    let metadata = fs::metadata(&path).expect("sidecar metadata");
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert!(
        fs::read_dir(temp.path())
            .expect("fixture directory")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")),
        "no temporary file remains"
    );
    let read = SourceCache::read(&path, "store-1");
    assert_eq!(read.len(), 1);
    assert_eq!(
        read.lookup(
            "/private/tmp/oxide-home",
            "registry/cache/i/cfg-if-1.0.0.crate"
        )
        .map(|record| record.verified_at),
        Some(5)
    );
    assert_eq!(SourceCache::read(&path, "store-2").len(), 0);

    // An unknown field, a wrong schema, a noncanonical digest, an impossible
    // nanosecond field (including both i64 extremes, where `abs` overflowed)
    // and malformed JSON each make the whole document ignored. Every forged
    // document must differ from the valid one, so a substitution that stops
    // matching its target cannot silently turn this regression vacuous.
    let text = fs::read_to_string(&path).expect("sidecar text");
    let nanosecond = "\"archive_mtime_nanos\":5";
    let forgeries = [
        (
            "unknown field",
            text.replace("\"records\"", "\"extra\":1,\"records\""),
        ),
        (
            "wrong schema",
            text.replace("oxide.seed-sources/1", "oxide.seed-sources/2"),
        ),
        (
            "noncanonical digest",
            text.replace(
                &sample_digest().to_hex(),
                &sample_digest().to_hex().to_uppercase(),
            ),
        ),
        (
            "nanos at one second",
            text.replace(nanosecond, "\"archive_mtime_nanos\":1000000000"),
        ),
        (
            "nanos at minus one second",
            text.replace(nanosecond, "\"archive_mtime_nanos\":-1000000000"),
        ),
        (
            "nanos at i64::MIN",
            text.replace(nanosecond, "\"archive_mtime_nanos\":-9223372036854775808"),
        ),
        (
            "nanos at i64::MAX",
            text.replace(nanosecond, "\"archive_mtime_nanos\":9223372036854775807"),
        ),
        ("malformed json", "{".to_owned()),
    ];
    for (label, forged) in forgeries {
        if label != "malformed json" {
            assert_ne!(forged, text, "{label} did not change the document");
        }
        fs::write(&path, &forged).expect("forge sidecar");
        assert_eq!(
            SourceCache::read(&path, "store-1").len(),
            0,
            "{label}: {forged}"
        );
    }

    // The same extremes are refused on insert, without a sidecar round trip.
    for nanos in [i64::MIN, i64::MAX, -1_000_000_000, 1_000_000_000] {
        let mut record = sample_record("registry/cache/i/extreme.crate", 1);
        record.archive_mtime_nanos = nanos;
        let mut probe = SourceCache::new("store-1").expect("cache");
        assert_eq!(probe.insert(record), Err(Refusal::CacheInvalid), "{nanos}");
        assert!(probe.is_empty());
    }

    // A symlinked sidecar is not read.
    fs::write(&path, text.as_bytes()).expect("restore sidecar");
    let target = temp.path().join("target.json");
    fs::rename(&path, &target).expect("move sidecar");
    symlink(&target, &path).expect("symlink sidecar");
    assert_eq!(SourceCache::read(&path, "store-1").len(), 0);
    fs::remove_file(&path).expect("remove sidecar link");

    // An oversized file is refused without being read.
    fs::write(&path, b"").expect("create sidecar");
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open sidecar")
        .set_len(MAX_CACHE_BYTES + 1)
        .expect("oversize sidecar");
    assert_eq!(SourceCache::read(&path, "store-1").len(), 0);

    // More records than the cap is ignored.
    let mut forged =
        String::from("{\"schema\":\"oxide.seed-sources/1\",\"store_id\":\"store-1\",\"records\":[");
    let record = serde_json::to_string(&sample_record("a.crate", 1)).expect("record json");
    for index in 0..=MAX_CACHE_RECORDS {
        if index > 0 {
            forged.push(',');
        }
        forged.push_str(&record);
    }
    forged.push_str("]}");
    fs::write(&path, forged.as_bytes()).expect("forge record cap");
    assert_eq!(SourceCache::read(&path, "store-1").len(), 0);

    // The in-memory list evicts the least recently verified record.
    let mut bounded = SourceCache::new("store-1").expect("cache");
    for index in 0..=MAX_CACHE_RECORDS {
        bounded
            .insert(sample_record(
                &format!("registry/cache/i/{index}.crate"),
                u64::try_from(index).expect("index"),
            ))
            .expect("insert");
    }
    assert_eq!(bounded.len(), MAX_CACHE_RECORDS);
    assert!(bounded
        .lookup("/private/tmp/oxide-home", "registry/cache/i/0.crate")
        .is_none());
    assert!(bounded
        .lookup(
            "/private/tmp/oxide-home",
            &format!("registry/cache/i/{MAX_CACHE_RECORDS}.crate")
        )
        .is_some());
}

#[test]
fn an_oversized_extension_payload_is_bounded_by_the_stream_cap() {
    // A GNU long-name and a PAX extension header each declare 1 GiB but carry
    // 64 KiB. The `tar` reader decodes extension payloads internally before it
    // yields an entry, so the bounded tar stream is what makes that work
    // finite: the low cap refuses at the bound instead of reading the claimed
    // size. With the design's default bound the same archive is refused as
    // unparseable, never read without limit.
    for (name, type_flag) in [
        (b"././@LongLink".as_slice(), b'L'),
        (b"PaxHeaders/x".as_slice(), b'x'),
    ] {
        let mut tar = raw_block_sized(name, type_flag, 1 << 30, &vec![b'a'; 64 * 1024]);
        tar.extend([0_u8; 1024]);
        let bytes = gzip(&tar);
        let low = SourceLimits {
            max_tar_bytes: 4096,
            ..SourceLimits::default()
        };
        assert_eq!(
            derive(Cursor::new(bytes.clone()), TARBALL, &low, Instant::now()).err(),
            Some(Refusal::ArchiveBudget),
            "extension type {}",
            char::from(type_flag)
        );
        assert!(
            derive(
                Cursor::new(bytes),
                TARBALL,
                &SourceLimits::default(),
                Instant::now()
            )
            .is_err(),
            "the default bound refuses the truncated extension"
        );
    }
}

#[test]
fn the_tree_walk_charges_entries_before_listing_them() {
    let fixture = PackageFixture::create("tree-enumeration", &simple_files());
    let mut cache = fixture.cache();
    // The archive has four entries, so this bound passes the archive
    // derivation; the extracted tree has six entries (four files and two
    // directories) and the walk refuses at the bound instead of listing it.
    let limits = SourceLimits {
        max_entries: 5,
        ..SourceLimits::default()
    };
    assert_eq!(
        fixture
            .locate()
            .verify(&fixture.checksum, &limits, &mut cache, 1)
            .err(),
        Some(Refusal::TreeBudget)
    );
    assert!(cache.is_empty());
}

#[test]
fn an_unbounded_registry_listing_refuses() {
    let temp = TempDir::new("index-limit");
    let cargo_home = temp.path().join("cargo-home");
    let cache = cargo_home.join("registry").join("cache");
    fs::create_dir_all(&cache).expect("cache root");
    for index in 0..=super::MAX_REGISTRY_INDEX_ENTRIES {
        fs::create_dir(cache.join(format!("index-{index}"))).expect("index directory");
    }
    assert_eq!(
        RegistrySource::locate(&cargo_home, TARBALL).err(),
        Some(Refusal::RegistryIndexLimit)
    );
}

struct GitFixture {
    _temp: TempDir,
    root: PathBuf,
    revision: String,
}

impl GitFixture {
    fn new(label: &str, files: &[(&str, &[u8], u32)]) -> Self {
        let temp = TempDir::new(label);
        let root = temp.path().join("repo");
        fs::create_dir(&root).expect("repo directory");
        let mut fixture = Self {
            _temp: temp,
            root,
            revision: String::new(),
        };
        fixture.git(&["init", "-q", "."]);
        for (path, data, mode) in files {
            write_file(&fixture.root.join(path), data, *mode);
        }
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "-qm", "seed"]);
        // Cargo's own marker, untracked and never evidence.
        write_file(&fixture.root.join(".cargo-ok"), b"", 0o644);
        fixture.revision = fixture.output(&["rev-parse", "HEAD"]).trim().to_owned();
        fixture
    }

    fn git(&self, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args([
                "-c",
                "user.name=Oxide Test",
                "-c",
                "user.email=oxide@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13Z")
            .env("GIT_COMMITTER_DATE", "2005-04-07T22:13:13Z")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git fixture command")
    }

    fn output(&self, args: &[&str]) -> String {
        let output = self.git(args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("git output")
    }

    fn status(&self) -> String {
        self.output(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules=none",
        ])
    }

    fn verify(&self) -> Result<super::git::GitSource, GitRefusal> {
        GitVerifier::new().verify(&self.root, &self.revision, &SourceLimits::default())
    }
}

fn git_files() -> Vec<(&'static str, &'static [u8], u32)> {
    vec![
        (
            "Cargo.toml",
            b"[package]\nname = \"seedctl\"\n".as_slice(),
            0o644,
        ),
        ("src/lib.rs", b"pub fn seed() {}\n".as_slice(), 0o644),
        ("build.sh", b"#!/bin/sh\n".as_slice(), 0o755),
    ]
}

#[test]
fn a_git_checkout_is_verified_against_the_locked_tree_and_the_counterexamples_are_honest() {
    let fixture = GitFixture::new("git-positive", &git_files());
    let first = fixture.verify().expect("clean checkout");
    assert_eq!(first.revision, fixture.revision);
    assert_eq!(first.files, 3);
    assert_eq!(first.digest.to_hex().len(), 64);

    // A new mtime with unchanged content leaves the digest unchanged; the
    // producing-window mtime rule, not the digest, is what would drop it.
    let path = fixture.root.join("src/lib.rs");
    let before = fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    set_mtime(&path, SystemTime::now() + Duration::from_secs(120));
    let after = fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    assert_ne!(before, after);
    assert_eq!(fixture.status(), "?? .cargo-ok\0");
    let touched = fixture.verify().expect("same content, a new mtime");
    assert_eq!(touched.digest, first.digest);

    // The 0ca2bd7f shape: modify and then restore the old content. Present
    // cleanliness and the digest both look normal again, which is exactly why
    // the producing window records mtimes and requires none to be later.
    fs::write(&path, b"pub fn seeb() {}\n").expect("modify");
    fs::write(&path, b"pub fn seed() {}\n").expect("restore");
    assert_eq!(fixture.status(), "?? .cargo-ok\0");
    assert_eq!(
        fixture.verify().expect("restored content").digest,
        first.digest
    );

    // A same-size content change cannot hide behind present cleanliness: git
    // is told to assume the file unchanged, so its status is empty while the
    // locked tree's object id no longer matches the file's content.
    fixture.git(&["update-index", "--assume-unchanged", "src/lib.rs"]);
    fs::write(&path, b"pub fn seec() {}\n").expect("same-size modify");
    assert_eq!(fixture.status(), "?? .cargo-ok\0");
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Dirty));
    fixture.git(&["update-index", "--no-assume-unchanged", "src/lib.rs"]);
    fs::write(&path, b"pub fn seed() {}\n").expect("restore");
    fixture.verify().expect("restored");
}

#[test]
fn git_verification_refuses_dirty_unsupported_wrong_and_unreadable_checkouts() {
    let mut fixture = GitFixture::new("git-negatives", &git_files());

    // A dirty tracked file.
    fs::write(fixture.root.join("src/lib.rs"), b"pub fn changed() {}\n").expect("dirty");
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Dirty));
    fs::write(fixture.root.join("src/lib.rs"), b"pub fn seed() {}\n").expect("restore");

    // Any untracked file other than Cargo's marker.
    write_file(&fixture.root.join("stray.txt"), b"stray", 0o644);
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Dirty));
    fs::remove_file(fixture.root.join("stray.txt")).expect("remove stray");

    // A wrong locked revision.
    let other = "0".repeat(fixture.revision.len());
    assert_eq!(
        GitVerifier::new()
            .verify(&fixture.root, &other, &SourceLimits::default())
            .err(),
        Some(GitRefusal::WrongRevision)
    );
    // A short or non-hexagonal locked revision is not a lock this verifier
    // accepts at all.
    assert_eq!(
        GitVerifier::new()
            .verify(&fixture.root, "deadbeef", &SourceLimits::default())
            .err(),
        Some(GitRefusal::WrongRevision)
    );

    // A tracked symlink is a type the seed cannot copy.
    symlink("Cargo.toml", fixture.root.join("link")).expect("symlink");
    fixture.git(&["add", "link"]);
    fixture.git(&["commit", "-qm", "link"]);
    fixture.revision = fixture.output(&["rev-parse", "HEAD"]).trim().to_owned();
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Unsupported));
    fs::remove_file(fixture.root.join("link")).expect("remove symlink");
    fixture.git(&["rm", "-q", "--cached", "link"]);
    fixture.git(&["commit", "-qm", "no link"]);
    fixture.revision = fixture.output(&["rev-parse", "HEAD"]).trim().to_owned();

    // A tracked file the verifier cannot read refuses the checkout. git's
    // own status cannot verify that file either, so the observed token is
    // `Dirty`; either way the package is ineligible.
    fs::set_permissions(
        fixture.root.join("src/lib.rs"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("unreadable file");
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Dirty));
    fs::set_permissions(
        fixture.root.join("src/lib.rs"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("readable file");
    fixture.verify().expect("readable again");

    // An unreadable repository cannot be verified at all.
    fs::set_permissions(fixture.root.join(".git"), fs::Permissions::from_mode(0o000))
        .expect("unreadable repository");
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Unreadable));
    fs::set_permissions(fixture.root.join(".git"), fs::Permissions::from_mode(0o755))
        .expect("readable repository");

    // An executable-bit change is a change.
    fs::set_permissions(
        fixture.root.join("src/lib.rs"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("executable");
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Dirty));
}

#[test]
fn a_submodule_bearing_checkout_is_ineligible() {
    let mut fixture = GitFixture::new("git-submodule", &git_files());
    // A child repository, committed, then added as a real submodule so the
    // parent status is clean.
    let child = fixture._temp.path().join("child");
    fs::create_dir(&child).expect("child directory");
    let child_git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&child)
            .args([
                "-c",
                "user.name=Oxide Test",
                "-c",
                "user.email=oxide@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13Z")
            .env("GIT_COMMITTER_DATE", "2005-04-07T22:13:13Z")
            .env("LC_ALL", "C")
            .output()
            .expect("child git")
    };
    assert!(child_git(&["init", "-q", "."]).status.success());
    write_file(&child.join("child.txt"), b"child\n", 0o644);
    assert!(child_git(&["add", "-A"]).status.success());
    assert!(child_git(&["commit", "-qm", "child"]).status.success());
    let output = Command::new("git")
        .arg("-C")
        .arg(&fixture.root)
        .args([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "user.name=Oxide Test",
            "-c",
            "user.email=oxide@example.invalid",
        ])
        .args(["submodule", "add", "-q"])
        .arg(&child)
        .arg("sub")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("LC_ALL", "C")
        .output()
        .expect("submodule add");
    assert!(
        output.status.success(),
        "submodule add: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fixture.git(&["commit", "-qm", "submodule"]);
    fixture.revision = fixture.output(&["rev-parse", "HEAD"]).trim().to_owned();
    assert_eq!(fixture.status(), "?? .cargo-ok\0", "the submodule is clean");
    assert_eq!(fixture.verify().err(), Some(GitRefusal::Submodule));
}

#[test]
fn every_git_call_uses_the_bounded_sanitized_protocol() {
    let temp = TempDir::new("fake-git");
    let root = temp.path().join("repo");
    fs::create_dir(&root).expect("fake repo");
    write_file(&root.join("file.txt"), b"x", 0o644);
    let object = "a".repeat(40);
    let log = temp.path().join("calls.log");
    let program = temp.path().join("git");
    let script = format!(
        "#!/bin/sh\nLOG='{log}'\nfor arg in \"$@\"; do printf 'ARG:%s\\n' \"$arg\" >> \"$LOG\"; done\n\
printf 'LOCKS:%s\\n' \"${{GIT_OPTIONAL_LOCKS-unset}}\" >> \"$LOG\"\n\
printf 'PROMPT:%s\\n' \"${{GIT_TERMINAL_PROMPT-unset}}\" >> \"$LOG\"\n\
printf 'DIR:%s\\n' \"${{GIT_DIR-unset}}\" >> \"$LOG\"\n\
printf 'WORKTREE:%s\\n' \"${{GIT_WORK_TREE-unset}}\" >> \"$LOG\"\n\
printf 'INDEX:%s\\n' \"${{GIT_INDEX_FILE-unset}}\" >> \"$LOG\"\n\
printf 'CONFIG:%s\\n' \"${{GIT_CONFIG_COUNT-unset}}\" >> \"$LOG\"\n\
while [ \"$#\" -gt 0 ]; do case \"$1\" in -c) shift 2 ;; -C) shift 2 ;; *) break ;; esac; done\n\
case \"$1\" in\n\
rev-parse) printf '%s\\n' '{object}' ;;\n\
status) : ;;\n\
ls-tree) printf '100644 blob %s %s\\tfile.txt' '{object}' '1'; printf '\\0' ;;\n\
hash-object) cat > /dev/null; printf '%s\\n' '{object}' ;;\n\
*) exit 9 ;;\n\
esac\n",
        log = log.display()
    );
    write_file(&program, script.as_bytes(), 0o755);
    let verified = GitVerifier::for_test(
        program,
        0,
        GIT_CALL_TIME,
        GIT_TOTAL_TIME,
        GIT_SETTLE_TIME,
        MAX_GIT_STDOUT_BYTES,
    )
    .verify(&root, &object, &SourceLimits::default())
    .unwrap_or_else(|error| {
        panic!(
            "fake verification: {error:?}\n{}",
            fs::read_to_string(&log).unwrap_or_default()
        )
    });
    assert_eq!(verified.files, 1);

    let root_text = root.display().to_string();
    let calls: [[&str; 4]; 4] = [
        ["rev-parse", "--verify", "HEAD", ""],
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        ["ls-tree", "-r", "-l", "-z"],
        ["hash-object", "--no-filters", "--stdin-paths", ""],
    ];
    let mut expected = String::new();
    let prefix = [
        "-c",
        "submodule.recurse=0",
        "-c",
        "core.fsmonitor=false",
        "-C",
    ];
    let status_tail = ["--no-renames", "--ignore-submodules=none"];
    for (index, call) in calls.iter().enumerate() {
        for argument in prefix {
            expected.push_str(&format!("ARG:{argument}\n"));
        }
        expected.push_str(&format!("ARG:{root_text}\n"));
        for argument in call.iter().filter(|argument| !argument.is_empty()) {
            expected.push_str(&format!("ARG:{argument}\n"));
        }
        if index == 1 {
            for argument in status_tail {
                expected.push_str(&format!("ARG:{argument}\n"));
            }
        }
        if index == 2 {
            expected.push_str("ARG:HEAD\n");
        }
        expected
            .push_str("LOCKS:0\nPROMPT:0\nDIR:unset\nWORKTREE:unset\nINDEX:unset\nCONFIG:unset\n");
    }
    assert_eq!(
        fs::read_to_string(&log).expect("call log"),
        expected,
        "every call's argv and environment"
    );
}

#[test]
fn git_command_sets_the_reviewed_environment_and_clears_inherited_roots() {
    let command = git_command(
        Path::new("/fake/git"),
        Path::new("/repo"),
        &["rev-parse", "--verify", "HEAD"],
    );
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "-c",
            "submodule.recurse=0",
            "-c",
            "core.fsmonitor=false",
            "-C",
            "/repo",
            "rev-parse",
            "--verify",
            "HEAD",
        ]
    );
    let mut environment: Vec<(String, Option<String>)> = command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    environment.sort();
    assert!(environment.contains(&("GIT_OPTIONAL_LOCKS".to_owned(), Some("0".to_owned()))));
    assert!(environment.contains(&("GIT_TERMINAL_PROMPT".to_owned(), Some("0".to_owned()))));
    for cleared in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        assert!(
            environment.contains(&(cleared.to_owned(), None)),
            "{cleared} is removed"
        );
    }
    for name in [
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_WORK_TREE",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
        "GIT_CONFIG_NOSYSTEM",
    ] {
        assert!(clears(OsStr::new(name)), "{name}");
    }
    for name in [
        "GIT_OPTIONAL_LOCKS",
        "GIT_TERMINAL_PROMPT",
        "GITCONFIG",
        "PATH",
    ] {
        assert!(!clears(OsStr::new(name)), "{name}");
    }
}

#[test]
fn a_fsmonitor_hook_never_runs_under_the_seeded_verification() {
    let fixture = GitFixture::new("git-fsmonitor", &git_files());
    let marker = fixture._temp.path().join("marker");
    let hook = fixture._temp.path().join("fsmonitor.sh");
    write_file(
        &hook,
        format!(
            "#!/bin/sh\ntouch '{}'\nprintf '{{\"version\":1}}'\n",
            marker.display()
        )
        .as_bytes(),
        0o755,
    );
    fixture.git(&["config", "core.fsmonitor", &hook.display().to_string()]);

    // The control: plain git does run the configured hook.
    let control = Command::new("git")
        .arg("-C")
        .arg(&fixture.root)
        .args(["status", "--porcelain"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("plain git status");
    let _ = control;
    assert!(marker.exists(), "the control hook ran");
    fs::remove_file(&marker).expect("clear marker");

    // Every seeded call passes `-c core.fsmonitor=false`.
    fixture
        .verify()
        .expect("verification under a configured hook");
    assert!(!marker.exists(), "the verification ran no fsmonitor hook");
}

#[test]
fn git_budget_exhaustion_and_unconfirmed_closure_refuse() {
    let temp = TempDir::new("git-budget");
    let root = temp.path().join("repo");
    fs::create_dir(&root).expect("fake repo");
    let object = "a".repeat(40);
    let limits = SourceLimits::default();

    // Call count.
    let quiet = temp.path().join("quiet-git");
    write_file(&quiet, b"#!/bin/sh\nexit 0\n", 0o755);
    let mut exhausted = GitVerifier::for_test(
        quiet.clone(),
        MAX_GIT_CALLS,
        GIT_CALL_TIME,
        GIT_TOTAL_TIME,
        GIT_SETTLE_TIME,
        MAX_GIT_STDOUT_BYTES,
    );
    assert_eq!(
        exhausted.verify(&root, &object, &limits).err(),
        Some(GitRefusal::Budget)
    );

    // Output bytes.
    let spam = temp.path().join("spam");
    write_file(&spam, &vec![b'x'; 4096], 0o644);
    let noisy = temp.path().join("noisy-git");
    write_file(
        &noisy,
        format!("#!/bin/sh\ncat '{}'\n", spam.display()).as_bytes(),
        0o755,
    );
    let mut loud = GitVerifier::for_test(
        noisy,
        0,
        GIT_CALL_TIME,
        GIT_TOTAL_TIME,
        GIT_SETTLE_TIME,
        1024,
    );
    assert_eq!(
        loud.verify(&root, &object, &limits).err(),
        Some(GitRefusal::Budget)
    );

    // A child that does not finish within its call clock.
    let slow = temp.path().join("slow-git");
    write_file(&slow, b"#!/bin/sh\nexec sleep 5\n", 0o755);
    let mut hanging = GitVerifier::for_test(
        slow,
        0,
        Duration::from_millis(100),
        Duration::from_secs(2),
        Duration::from_millis(100),
        MAX_GIT_STDOUT_BYTES,
    );
    assert_eq!(
        hanging.verify(&root, &object, &limits).err(),
        Some(GitRefusal::Unconfirmed)
    );

    // A retained pipe held by a process the child started. The fixture
    // records that process so this test leaves nothing behind.
    let holder = temp.path().join("holder-git");
    let holder_pid = temp.path().join("holder.pid");
    write_file(
        &holder,
        format!(
            "#!/bin/sh\nsleep 2 &\necho $! > '{}'\nexit 0\n",
            holder_pid.display()
        )
        .as_bytes(),
        0o755,
    );
    let mut retained = GitVerifier::for_test(
        holder,
        0,
        GIT_CALL_TIME,
        GIT_TOTAL_TIME,
        Duration::from_millis(100),
        MAX_GIT_STDOUT_BYTES,
    );
    assert_eq!(
        retained.verify(&root, &object, &limits).err(),
        Some(GitRefusal::Unconfirmed)
    );
    if let Ok(pid) = fs::read_to_string(&holder_pid)
        .unwrap_or_default()
        .trim()
        .parse::<i32>()
    {
        // SAFETY: the fixture wrote the PID of the `sleep` it started; a
        // failed kill means the process already ended.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }

    // A missing program and a failing program are unreadable, not a cold skip.
    let mut missing = GitVerifier::for_test(
        temp.path().join("no-such-git"),
        0,
        GIT_CALL_TIME,
        GIT_TOTAL_TIME,
        GIT_SETTLE_TIME,
        MAX_GIT_STDOUT_BYTES,
    );
    assert_eq!(
        missing.verify(&root, &object, &limits).err(),
        Some(GitRefusal::Unreadable)
    );
    let failing = temp.path().join("failing-git");
    write_file(&failing, b"#!/bin/sh\nexit 1\n", 0o755);
    let mut failed = GitVerifier::for_test(
        failing,
        0,
        GIT_CALL_TIME,
        GIT_TOTAL_TIME,
        GIT_SETTLE_TIME,
        MAX_GIT_STDOUT_BYTES,
    );
    assert_eq!(
        failed.verify(&root, &object, &limits).err(),
        Some(GitRefusal::Unreadable)
    );

    // The checkout's own entry and content bounds.
    let fixture = GitFixture::new("git-bounds", &git_files());
    assert_eq!(
        GitVerifier::new()
            .verify(
                &fixture.root,
                &fixture.revision,
                &SourceLimits {
                    max_content_bytes: 1,
                    ..SourceLimits::default()
                }
            )
            .err(),
        Some(GitRefusal::Budget)
    );
    assert_eq!(
        GitVerifier::new()
            .verify(
                &fixture.root,
                &fixture.revision,
                &SourceLimits {
                    max_entries: 1,
                    ..SourceLimits::default()
                }
            )
            .err(),
        Some(GitRefusal::Budget)
    );
}
