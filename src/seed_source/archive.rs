//! Strict, bounded reader for one Cargo registry `.crate` archive.
//!
//! Owning design: `docs/knowledge/contracts/cold-slot-seeding.md` §4.3. The
//! expected extracted tree is derived from the authenticated archive bytes,
//! entry by entry, and never from `.cargo-checksum.json` or from the extracted
//! tree's own declaration.
//!
//! Grammar: regular files and directories only. `ustar`, pax and GNU long
//! names are accepted by the `tar` reader. Symlinks, hard links, device nodes,
//! FIFOs, sparse entries, duplicate paths, absolute paths, `.`/`..` components,
//! a wrong top-level archive prefix and any unparseable or truncated entry
//! refuse. After the tar end-of-archive marker only zero padding is accepted,
//! so an entry or payload hidden past that marker cannot bypass the grammar,
//! and the archive must be one gzip member: a second member or any trailing
//! compressed payload refuses rather than being hashed without being decoded.
//! Entry count, archive bytes and derived content are bounded and the deadline
//! is checked between entries, so a hostile archive cannot make the verifier do
//! unbounded work.

use super::digest::{ContentHash, EntryKind, SourceTree};
use super::{Refusal, SourceLimits};
use flate2::bufread::GzDecoder;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::io::{self, BufReader, Read};
use std::rc::Rc;
use std::time::Instant;
use tar::EntryType;

/// Decompressed bytes after the tar end-of-archive marker that are accepted as
/// writer padding. Anything beyond this is an unparseable trailing payload.
const MAX_TRAILING_BYTES: u64 = 1024 * 1024;
const CHUNK: usize = 64 * 1024;

/// Decompressed tar-stream bytes the reader consumes: 512-byte headers, block
/// padding and, critically, the PAX/GNU extension payloads the `tar` reader
/// decodes internally before it yields an entry. The `tar` reader preallocates
/// at most 128 KiB for an extension but grows the buffer as it reads, so this
/// wrapper is what makes extension decoding finite and deadline-checked.
struct TarStream<R> {
    inner: R,
    bytes: u64,
    cap: u64,
    deadline: Instant,
    reason: Rc<Cell<Option<Refusal>>>,
}

impl<R: Read> TarStream<R> {
    fn new(inner: R, cap: u64, deadline: Instant) -> (Self, Rc<Cell<Option<Refusal>>>) {
        let reason = Rc::new(Cell::new(None));
        (
            Self {
                inner,
                bytes: 0,
                cap,
                deadline,
                reason: Rc::clone(&reason),
            },
            reason,
        )
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for TarStream<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.reason.get().is_some() {
            return Err(io::Error::other("the tar stream stopped"));
        }
        if Instant::now() > self.deadline {
            self.reason.set(Some(Refusal::Deadline));
            return Err(io::Error::other("the source deadline expired"));
        }
        let read = self.inner.read(buffer)?;
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if self.bytes > self.cap {
            self.reason.set(Some(Refusal::ArchiveBudget));
            return Err(io::Error::other("the tar stream exceeded its byte bound"));
        }
        Ok(read)
    }
}

/// Why a read through the bounded readers stopped: the tar stream's own bound
/// or deadline first, then the compressed-archive bound, else corruption.
struct Signals {
    compressed_overflow: Rc<Cell<bool>>,
    stream: Rc<Cell<Option<Refusal>>>,
}

impl Signals {
    fn refusal(&self) -> Refusal {
        if let Some(reason) = self.stream.get() {
            return reason;
        }
        if self.compressed_overflow.get() {
            Refusal::ArchiveBudget
        } else {
            Refusal::ArchiveCorrupt
        }
    }
}

/// One archive derivation: the expected extracted tree, the SHA-256 of the
/// archive bytes and the charged content.
pub(super) struct DerivedArchive {
    pub tree: SourceTree,
    /// SHA-256 over every byte of the archive file.
    pub sha256: [u8; 32],
    /// Sum of the regular-file sizes the archive declares.
    pub content_bytes: u64,
}

/// A reader that hashes every byte and refuses past the archive byte bound.
struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
    cap: u64,
    overflow: Rc<Cell<bool>>,
}

impl<R: Read> HashingReader<R> {
    fn new(inner: R, cap: u64) -> (Self, Rc<Cell<bool>>) {
        let overflow = Rc::new(Cell::new(false));
        (
            Self {
                inner,
                hasher: Sha256::new(),
                bytes: 0,
                cap,
                overflow: Rc::clone(&overflow),
            },
            overflow,
        )
    }

    fn finish(self) -> [u8; 32] {
        let output = self.hasher.finalize();
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&output);
        digest
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.overflow.get() {
            return Err(io::Error::other("the archive exceeded its byte bound"));
        }
        let read = self.inner.read(buffer)?;
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if self.bytes > self.cap {
            self.overflow.set(true);
            return Err(io::Error::other("the archive exceeded its byte bound"));
        }
        self.hasher.update(&buffer[..read]);
        Ok(read)
    }
}

/// Derive the expected tree from one archive.
pub(super) fn derive<R: Read>(
    source: R,
    tarball_name: &str,
    limits: &SourceLimits,
    started: Instant,
) -> Result<DerivedArchive, Refusal> {
    let (mut hashing, compressed_overflow) = HashingReader::new(source, limits.max_archive_bytes);
    let deadline = started + limits.deadline;
    let (stream, stream_reason) = TarStream::new(
        GzDecoder::new(BufReader::new(&mut hashing)),
        limits.max_tar_bytes,
        deadline,
    );
    let signals = Signals {
        compressed_overflow,
        stream: stream_reason,
    };
    let mut tree = SourceTree::new();
    let mut content_bytes: u64 = 0;
    let mut entries: u64 = 0;
    let mut root_seen = false;
    let mut archive = tar::Archive::new(stream);
    {
        let reader = archive.entries().map_err(|_| signals.refusal())?;
        for entry in reader {
            if Instant::now() > deadline {
                return Err(Refusal::Deadline);
            }
            let mut entry = entry.map_err(|_| signals.refusal())?;
            entries = entries.saturating_add(1);
            if entries > limits.max_entries {
                return Err(Refusal::ArchiveBudget);
            }
            let entry_type = entry.header().entry_type();
            let is_directory = match entry_type {
                EntryType::Regular => false,
                EntryType::Directory => true,
                EntryType::Link => return Err(Refusal::ArchiveGrammar("hardlink")),
                EntryType::Symlink => return Err(Refusal::ArchiveGrammar("symlink")),
                EntryType::Char | EntryType::Block => {
                    return Err(Refusal::ArchiveGrammar("device"));
                }
                EntryType::Fifo => return Err(Refusal::ArchiveGrammar("fifo")),
                EntryType::GNUSparse => return Err(Refusal::ArchiveGrammar("sparse")),
                _ => return Err(Refusal::ArchiveGrammar("unsupported")),
            };
            let raw_path = entry.path_bytes().to_vec();
            let path =
                std::str::from_utf8(&raw_path).map_err(|_| Refusal::ArchiveGrammar("non-utf8"))?;
            if path.starts_with('/') {
                return Err(Refusal::ArchiveGrammar("absolute"));
            }
            let Some(rest) = strip_prefix(path, tarball_name).map(str::to_owned) else {
                return Err(Refusal::ArchiveGrammar("prefix"));
            };
            if rest.is_empty() {
                // The archive's own top-level directory, which is the extracted
                // tree's root and therefore never an entry.
                if !is_directory {
                    return Err(Refusal::ArchiveGrammar("prefix"));
                }
                if root_seen {
                    return Err(Refusal::ArchiveGrammar("duplicate"));
                }
                root_seen = true;
                continue;
            }
            if is_directory {
                let rest = rest.strip_suffix('/').unwrap_or(&rest).to_owned();
                if rest.is_empty() {
                    if root_seen {
                        return Err(Refusal::ArchiveGrammar("duplicate"));
                    }
                    root_seen = true;
                    continue;
                }
                tree.insert_explicit(&rest, EntryKind::Directory, limits.max_path_bytes)
                    .map_err(Refusal::ArchiveGrammar)?;
                continue;
            }
            let mode = entry.header().mode().map_err(|_| Refusal::ArchiveCorrupt)?;
            let executable = mode & 0o111 != 0;
            let declared = entry.size();
            let charged = content_bytes
                .checked_add(declared)
                .ok_or(Refusal::ArchiveBudget)?;
            if charged > limits.max_derived_bytes {
                return Err(Refusal::ArchiveBudget);
            }
            let (size, hash) = hash_entry(&mut entry, declared, deadline, &signals)?;
            if size != declared {
                return Err(Refusal::ArchiveCorrupt);
            }
            content_bytes = charged;
            tree.insert_explicit(
                &rest,
                EntryKind::File {
                    executable,
                    size,
                    content: ContentHash::Sha256(hash),
                },
                limits.max_path_bytes,
            )
            .map_err(Refusal::ArchiveGrammar)?;
        }
    }
    // The pinned iterator stops at the first zero header, so everything after
    // the end-of-archive marker is checked here: only zero writer padding is
    // accepted, and a non-zero byte or an over-long tail refuses, so an entry
    // or payload hidden past the marker cannot bypass the entry grammar. The
    // archive's checksum covers the whole file, so the rest of the gzip stream
    // and any trailing bytes are still consumed for the hash.
    let mut decoder = archive.into_inner();
    let mut trailing: u64 = 0;
    let mut buffer = [0_u8; CHUNK];
    loop {
        if Instant::now() > deadline {
            return Err(Refusal::Deadline);
        }
        let read = decoder.read(&mut buffer).map_err(|_| signals.refusal())?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().any(|byte| *byte != 0) {
            return Err(Refusal::ArchiveGrammar("trailing"));
        }
        trailing = trailing.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if trailing > MAX_TRAILING_BYTES {
            return Err(Refusal::ArchiveBudget);
        }
    }
    // The decoder decodes exactly one gzip member. A `.crate` is one member,
    // so anything the buffer still holds, and anything left in the compressed
    // stream after it, refuses: a second member or another compressed payload
    // would otherwise be authenticated by the lock checksum without ever being
    // decoded or subjected to the entry grammar. Bytes already pulled into the
    // buffer were hashed as the reader supplied them, and the final drain
    // hashes the rest, so a valid single member is still hashed whole.
    let buffered = decoder.into_inner().into_inner();
    if !buffered.buffer().is_empty() {
        return Err(Refusal::ArchiveGrammar("member"));
    }
    let inner = buffered.into_inner();
    loop {
        if Instant::now() > deadline {
            return Err(Refusal::Deadline);
        }
        let read = inner.read(&mut buffer).map_err(|_| signals.refusal())?;
        if read == 0 {
            break;
        }
        return Err(Refusal::ArchiveGrammar("member"));
    }
    let sha256 = hashing.finish();
    Ok(DerivedArchive {
        tree,
        sha256,
        content_bytes,
    })
}

fn strip_prefix<'a>(path: &'a str, tarball_name: &str) -> Option<&'a str> {
    if path == tarball_name {
        Some("")
    } else {
        path.strip_prefix(tarball_name)?.strip_prefix('/')
    }
}

/// Read one regular entry under its declared size, returning exactly the bytes
/// the archive carries.
fn hash_entry<R: Read>(
    entry: &mut R,
    declared: u64,
    deadline: Instant,
    signals: &Signals,
) -> Result<(u64, [u8; 32]), Refusal> {
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut buffer = [0_u8; CHUNK];
    loop {
        if Instant::now() > deadline {
            return Err(Refusal::Deadline);
        }
        let read = entry.read(&mut buffer).map_err(|_| signals.refusal())?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > declared {
            return Err(Refusal::ArchiveCorrupt);
        }
        hasher.update(&buffer[..read]);
    }
    let output = hasher.finalize();
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&output);
    Ok((total, digest))
}
