//! Bounded walk of an extracted source tree and the archive-to-tree comparison.
//!
//! Owning design: `docs/knowledge/contracts/cold-slot-seeding.md` §4.3. The
//! walk hashes the bytes on disk, refuses every entry type the archive grammar
//! refuses, checks the deadline between filesystem calls, and charges entries
//! and content against the same finite bounds the archive derivation uses.
//! Cargo's own `.cargo-ok` marker at the tree root is skipped: it is never
//! evidence, and the archive never contains it.

use super::digest::{ContentHash, EntryKind, SourceTree};
use super::{Refusal, SourceLimits};
use sha2::{Digest, Sha256};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

const CHUNK: usize = 64 * 1024;
/// Cargo's post-unpack marker at the root of an extracted registry tree.
const CARGO_OK: &str = ".cargo-ok";

/// A file's immutable identity as the verification cache records it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
}

impl FileIdentity {
    pub(super) fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            mtime_secs: metadata.mtime(),
            mtime_nanos: metadata.mtime_nsec(),
        }
    }
}

/// An opened regular file and the identity of the descriptor's own inode, so
/// the recorded identity belongs to the bytes that are read.
pub(super) struct OpenedFile {
    pub file: File,
    pub identity: FileIdentity,
}

/// Open one path as a regular file without following a final symlink, and
/// return the descriptor's own identity.
pub(super) fn open_regular(path: &Path) -> Result<OpenedFile, Refusal> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| Refusal::ArchiveUnreadable)?;
    let metadata = file.metadata().map_err(|_| Refusal::ArchiveUnreadable)?;
    if !metadata.file_type().is_file() {
        return Err(Refusal::ArchiveUnreadable);
    }
    Ok(OpenedFile {
        identity: FileIdentity::of(&metadata),
        file,
    })
}

/// The identity of the extracted tree's root directory, never following a
/// symlink.
pub(super) fn tree_identity(path: &Path) -> Result<FileIdentity, Refusal> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Refusal::MissingTree
        } else {
            Refusal::TreeUnreadable
        }
    })?;
    if !metadata.file_type().is_dir() {
        return Err(Refusal::TreeUnreadable);
    }
    Ok(FileIdentity::of(&metadata))
}

/// One extracted-tree walk: the canonical tree and its charged content.
pub(super) struct WalkedTree {
    pub tree: SourceTree,
    pub content_bytes: u64,
}

struct Pending {
    path: PathBuf,
    relative: String,
    depth: usize,
}

/// Walk one extracted registry tree. `.cargo-ok` at the root is skipped; every
/// other entry is part of the tree.
pub(super) fn walk_extracted_tree(
    root: &Path,
    limits: &SourceLimits,
    started: Instant,
) -> Result<WalkedTree, Refusal> {
    tree_identity(root)?;
    let deadline = started + limits.deadline;
    let mut tree = SourceTree::new();
    let mut content_bytes: u64 = 0;
    let mut entries: u64 = 0;
    let mut stack = vec![Pending {
        path: root.to_path_buf(),
        relative: String::new(),
        depth: 0,
    }];
    while let Some(pending) = stack.pop() {
        // Enumerate one entry at a time and charge it before touching it, so a
        // directory with more entries than the bound refuses without listing or
        // allocating the whole directory.
        for entry in fs::read_dir(&pending.path).map_err(|_| Refusal::TreeUnreadable)? {
            if Instant::now() > deadline {
                return Err(Refusal::Deadline);
            }
            let entry = entry.map_err(|_| Refusal::TreeUnreadable)?;
            entries = entries.saturating_add(1);
            if entries > limits.max_entries {
                return Err(Refusal::TreeBudget);
            }
            let name = entry
                .file_name()
                .to_str()
                .ok_or(Refusal::TreeGrammar("non-utf8"))?
                .to_owned();
            let relative = if pending.relative.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", pending.relative)
            };
            let path = pending.path.join(&name);
            let metadata = fs::symlink_metadata(&path).map_err(|_| Refusal::TreeUnreadable)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(Refusal::TreeGrammar("symlink"));
            }
            if file_type.is_dir() {
                if pending.depth >= limits.max_depth {
                    return Err(Refusal::TreeGrammar("depth"));
                }
                tree.insert_explicit(&relative, EntryKind::Directory, limits.max_path_bytes)
                    .map_err(Refusal::TreeGrammar)?;
                stack.push(Pending {
                    path,
                    relative,
                    depth: pending.depth + 1,
                });
                continue;
            }
            if !file_type.is_file() {
                return Err(Refusal::TreeGrammar("special"));
            }
            if pending.depth == 0 && name == CARGO_OK {
                continue;
            }
            let charged = content_bytes
                .checked_add(metadata.size())
                .ok_or(Refusal::TreeBudget)?;
            if charged > limits.max_content_bytes {
                return Err(Refusal::TreeBudget);
            }
            let (size, hash) = hash_file(&path, &metadata, deadline)?;
            content_bytes = charged;
            let executable = metadata.permissions().mode() & 0o111 != 0;
            tree.insert_explicit(
                &relative,
                EntryKind::File {
                    executable,
                    size,
                    content: ContentHash::Sha256(hash),
                },
                limits.max_path_bytes,
            )
            .map_err(Refusal::TreeGrammar)?;
        }
    }
    Ok(WalkedTree {
        tree,
        content_bytes,
    })
}

/// Hash one regular file, requiring the opened descriptor to be the inode the
/// walk listed and the bytes read to be exactly its recorded size.
pub(super) fn hash_file(
    path: &Path,
    listed: &Metadata,
    deadline: Instant,
) -> Result<(u64, [u8; 32]), Refusal> {
    let mut opened = open_regular(path).map_err(|_| Refusal::TreeUnreadable)?;
    let identity = opened.identity;
    if identity.device != listed.dev() || identity.inode != listed.ino() {
        return Err(Refusal::TreeGrammar("changed"));
    }
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut buffer = [0_u8; CHUNK];
    loop {
        if Instant::now() > deadline {
            return Err(Refusal::Deadline);
        }
        let read = opened
            .file
            .read(&mut buffer)
            .map_err(|_| Refusal::TreeUnreadable)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > identity.size {
            return Err(Refusal::TreeGrammar("changed"));
        }
        hasher.update(&buffer[..read]);
    }
    if total != identity.size {
        return Err(Refusal::TreeGrammar("changed"));
    }
    let output = hasher.finalize();
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&output);
    Ok((total, digest))
}

/// Compare the archive-derived tree with the walked tree exactly. A mode,
/// type, size or content difference, or an entry present on only one side,
/// fails closed.
pub(super) fn compare(derived: &SourceTree, walked: &SourceTree) -> Result<(), Refusal> {
    let mut derived_entries = derived.iter().peekable();
    let mut walked_entries = walked.iter().peekable();
    loop {
        let next = match (derived_entries.peek(), walked_entries.peek()) {
            (None, None) => return Ok(()),
            (None, Some(_)) => return Err(Refusal::TreeMismatch("extra")),
            (Some(_), None) => return Err(Refusal::TreeMismatch("missing")),
            (Some((derived_path, derived_kind)), Some((walked_path, walked_kind))) => {
                match derived_path.cmp(walked_path) {
                    std::cmp::Ordering::Less => return Err(Refusal::TreeMismatch("missing")),
                    std::cmp::Ordering::Greater => return Err(Refusal::TreeMismatch("extra")),
                    std::cmp::Ordering::Equal => (derived_kind, walked_kind),
                }
            }
        };
        let (derived_kind, walked_kind) = next;
        match (derived_kind, walked_kind) {
            (EntryKind::Directory, EntryKind::Directory) => {}
            (
                EntryKind::File {
                    executable: derived_executable,
                    size: derived_size,
                    content: derived_content,
                },
                EntryKind::File {
                    executable: walked_executable,
                    size: walked_size,
                    content: walked_content,
                },
            ) => {
                if derived_executable != walked_executable {
                    return Err(Refusal::TreeMismatch("executable"));
                }
                if derived_size != walked_size {
                    return Err(Refusal::TreeMismatch("size"));
                }
                if derived_content != walked_content {
                    return Err(Refusal::TreeMismatch("content"));
                }
            }
            _ => return Err(Refusal::TreeMismatch("type")),
        }
        derived_entries.next();
        walked_entries.next();
    }
}
