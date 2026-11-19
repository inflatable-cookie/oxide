//! Pure and tiny-fixture regressions for the slice-1 source verification
//! (oxide#166).
//!
//! Archive fixtures are built in memory; extracted trees and caches are tiny
//! directories under the ordinary test temp directory (`TMPDIR`, on the
//! selected volume). Every fixture is owned and removed by its guard. No
//! Cargo, pool, lease, install or removable volume is involved.

use super::archive::derive;
use super::cache::{CacheRecord, SourceCache, MAX_CACHE_BYTES, MAX_CACHE_RECORDS};
use super::digest::{DigestScheme, SourceTree, TreeDigest, MAX_PATH_BYTES};
use super::{Refusal, RegistrySource, SourceLimits, VerifiedRegistrySource};
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::fs;
use std::fs::FileTimes;
use std::io::{Cursor, Write};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
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
fn nothing_non_padding_may_follow_the_tar_end_marker() {
    // A valid entry, then the end-of-archive marker, then bytes the pinned
    // iterator never yields. Zero writer padding stays accepted; a hidden
    // entry, a hidden header or a single non-zero padding byte refuses.
    let entry = raw_block(b"cfg-if-1.0.0/Cargo.toml", b'0', b"[package]\n");
    let valid = |tail: Vec<u8>| {
        let mut tar = entry.clone();
        tar.extend([0_u8; 1024]);
        tar.extend(tail);
        gzip(&tar)
    };

    // Pure zero padding (as every tar writer emits) is accepted.
    let padded = valid(vec![0_u8; 8192]);
    derive(
        Cursor::new(padded),
        TARBALL,
        &SourceLimits::default(),
        Instant::now(),
    )
    .expect("zero padding after the end marker");

    // A whole hidden entry after the marker is refused.
    let hidden = raw_block(b"cfg-if-1.0.0/hidden.rs", b'0', b"pub fn hidden() {}\n");
    assert_eq!(
        archive_error(valid(hidden.clone()), TARBALL),
        Some(Refusal::ArchiveGrammar("trailing")),
        "a hidden entry after the end marker"
    );
    // A single non-zero byte in the padding is refused.
    assert_eq!(
        archive_error(
            valid(vec![0_u8; 600].into_iter().chain([1]).collect()),
            TARBALL
        ),
        Some(Refusal::ArchiveGrammar("trailing")),
        "a non-zero padding byte"
    );
    // A hidden payload that begins on a block boundary is still refused.
    let mut block_aligned = vec![0_u8; 512];
    block_aligned.extend_from_slice(&hidden);
    assert_eq!(
        archive_error(valid(block_aligned), TARBALL),
        Some(Refusal::ArchiveGrammar("trailing")),
        "a block-aligned hidden payload"
    );
}

#[test]
fn a_second_gzip_member_or_compressed_tail_is_refused() {
    // The decoder handles exactly one gzip member. Each member below is a
    // valid archive on its own, so the concatenation is precisely the case a
    // lock checksum could authenticate while the strict grammar never saw the
    // hidden entry; it now refuses at the member boundary.
    let first = build_archive(&[(
        format!("{TARBALL}/Cargo.toml"),
        Spec::File {
            data: b"[package]\n",
            mode: 0o644,
        },
    )]);
    let second = build_archive(&[(
        format!("{TARBALL}/hidden.rs"),
        Spec::File {
            data: b"pub fn hidden() {}\n",
            mode: 0o644,
        },
    )]);
    assert!(
        derive(
            Cursor::new(first.clone()),
            TARBALL,
            &SourceLimits::default(),
            Instant::now()
        )
        .is_ok(),
        "the first member alone is a valid archive"
    );
    assert!(
        derive(
            Cursor::new(second.clone()),
            TARBALL,
            &SourceLimits::default(),
            Instant::now()
        )
        .is_ok(),
        "the hidden member alone is a valid archive"
    );

    let mut concatenated = first.clone();
    concatenated.extend_from_slice(&second);
    assert_ne!(
        sha256_hex(&concatenated),
        sha256_hex(&first),
        "a lock naming the concatenation authenticates the hidden bytes"
    );
    assert_eq!(
        archive_error(concatenated, TARBALL),
        Some(Refusal::ArchiveGrammar("member"))
    );

    // An empty second member is still a second member.
    let mut empty_member = first.clone();
    empty_member.extend_from_slice(&gzip(b""));
    assert_eq!(
        archive_error(empty_member, TARBALL),
        Some(Refusal::ArchiveGrammar("member"))
    );

    // Raw compressed payload after the member is refused the same way.
    let mut raw_tail = first;
    raw_tail.extend_from_slice(b"trailing");
    assert_eq!(
        archive_error(raw_tail, TARBALL),
        Some(Refusal::ArchiveGrammar("member"))
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

#[test]
fn a_foreign_owned_cache_document_is_ignored() {
    // §9.1: a foreign-owned sidecar is ignored, never adopted. A non-privileged
    // test cannot create an inode owned by another user, so the read path's
    // owner comparison is proved through its seam: the same document is
    // accepted for its real owner and refused for any other expected owner.
    let temp = TempDir::new("cache-owner");
    let path = temp.path().join("sources.json");
    let mut cache = SourceCache::new("store-1").expect("cache");
    cache
        .insert(sample_record("registry/cache/i/cfg-if-1.0.0.crate", 5))
        .expect("insert");
    cache.write(&path).expect("write");
    let owner = fs::metadata(&path).expect("sidecar metadata").uid();
    assert_eq!(SourceCache::read(&path, "store-1").len(), 1);
    assert_eq!(SourceCache::read_owned_by(&path, "store-1", owner).len(), 1);
    assert_eq!(
        SourceCache::read_owned_by(&path, "store-1", owner.wrapping_add(1)).len(),
        0,
        "a document owned by another user is ignored"
    );
}

#[test]
fn a_failed_temporary_creation_never_unlinks_a_foreign_entry() {
    // `create_new` reports a collision without opening the pre-existing entry;
    // the failure path must not remove it.
    let temp = TempDir::new("cache-collision");
    let path = temp.path().join("sources.json");
    let pre_existing = temp.path().join(".sources.json.tmp-collision");
    write_file(&pre_existing, b"not ours", 0o600);
    let cache = SourceCache::new("store-1").expect("cache");
    let bytes = cache.encode().expect("encode");
    assert_eq!(
        SourceCache::write_at(&path, temp.path(), &pre_existing, &bytes),
        Err(Refusal::CacheUnwritable)
    );
    assert_eq!(
        fs::read(&pre_existing).expect("the pre-existing entry survives"),
        b"not ours"
    );
    assert!(!path.exists(), "nothing was published");
}

#[test]
fn a_write_that_fails_after_creating_its_temporary_removes_only_its_own_temp() {
    // The temporary this call created is cleaned up; the destination (here a
    // directory that the rename cannot replace) is left exactly as it was.
    let temp = TempDir::new("cache-cleanup");
    let path = temp.path().join("sources.json");
    fs::create_dir(&path).expect("a directory at the destination");
    let temporary = temp.path().join(".sources.json.tmp-owned");
    let cache = SourceCache::new("store-1").expect("cache");
    let bytes = cache.encode().expect("encode");
    assert_eq!(
        SourceCache::write_at(&path, temp.path(), &temporary, &bytes),
        Err(Refusal::CacheUnwritable)
    );
    assert!(!temporary.exists(), "this call's temporary is removed");
    assert!(path.is_dir(), "the destination is untouched");
    assert_eq!(
        fs::read_dir(temp.path())
            .expect("fixture directory")
            .count(),
        1,
        "only the destination remains"
    );
}
