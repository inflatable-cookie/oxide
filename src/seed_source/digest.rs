//! Canonical source-tree entries and the canonical tree digest.
//!
//! Owning design: `docs/knowledge/contracts/cold-slot-seeding.md` §4.3. The
//! digest is a SHA-256 over sorted relative paths, the entry type, the
//! executable bit, the size and the content hash. It is computed from bytes the
//! verifier read, never from a tree's own declaration, and two derivations of
//! the same content produce the same digest regardless of the order the
//! entries were found in.
//!
//! Directory entries participate so an empty directory declared by an archive
//! cannot disappear silently; parents are implied by their files. Symlinks,
//! device nodes and every other entry type are refused before they reach this
//! module, so the only two kinds here are directories and regular files.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Longest accepted relative path in bytes.
pub const MAX_PATH_BYTES: usize = 4096;

/// Which content hash a tree's files carry.
///
/// Registry sources hash the file content with SHA-256. Git sources compare
/// against the object ids of the locked revision, which the locked checkout's
/// format fixes (SHA-1 or SHA-256), so the digest binds to the locked tree
/// rather than to bytes the checkout merely presents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestScheme {
    /// A registry tree derived from an authenticated `.crate` archive.
    RegistrySource,
    /// A git checkout's tracked tree at the locked revision.
    GitSource,
}

impl DigestScheme {
    fn domain(self) -> &'static [u8] {
        match self {
            Self::RegistrySource => b"oxide.source-tree/1\0",
            Self::GitSource => b"oxide.git-tree/1\0",
        }
    }
}

/// The content hash of one regular file, in the scheme its tree uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContentHash {
    /// SHA-256 of the file's bytes.
    Sha256([u8; 32]),
    /// A git object id (20 bytes for SHA-1 repositories, 32 for SHA-256).
    GitObject(Vec<u8>),
}

/// One canonical tree entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// A directory. Permission bits are not source content and are not kept.
    Directory,
    /// A regular file with its executable bit, size and content hash.
    File {
        executable: bool,
        size: u64,
        content: ContentHash,
    },
}

/// A canonical tree digest. The scheme is part of the hashed domain, so a
/// registry digest can never equal a git digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TreeDigest([u8; 32]);

impl TreeDigest {
    /// The raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hexadecimal, the only rendering the seed sidecars store.
    pub fn to_hex(self) -> String {
        hex_bytes(&self.0)
    }

    /// Parse exactly 64 hexadecimal characters. Uppercase input is accepted
    /// and normalized; any other length or character refuses.
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut digest = [0_u8; 32];
        for (index, pair) in bytes.chunks_exact(2).enumerate() {
            let high = hex_value(pair[0])?;
            let low = hex_value(pair[1])?;
            digest[index] = (high << 4) | low;
        }
        Some(Self(digest))
    }
}

/// Lowercase hexadecimal for a digest-sized byte string.
pub(super) fn hex_bytes(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(hex_digit(byte >> 4));
        text.push(hex_digit(byte & 0x0f));
    }
    text
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'a' + value - 10),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A canonical source tree: unique relative paths and their entries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceTree {
    entries: BTreeMap<String, EntryKind>,
}

impl SourceTree {
    /// An empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the tree holds no entry.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// One entry by relative path.
    pub fn get(&self, path: &str) -> Option<&EntryKind> {
        self.entries.get(path)
    }

    /// Entries in canonical (byte-sorted path) order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &EntryKind)> {
        self.entries
            .iter()
            .map(|(path, kind)| (path.as_str(), kind))
    }

    /// Insert one explicitly derived entry, failing closed on an invalid path,
    /// a duplicate path, or a file whose parent is already a file. Missing
    /// parent directories are added as directories.
    pub fn insert_explicit(
        &mut self,
        path: &str,
        kind: EntryKind,
        max_path_bytes: usize,
    ) -> Result<(), &'static str> {
        validate_path(path, max_path_bytes)?;
        if self.entries.contains_key(path) {
            return Err("duplicate");
        }
        if matches!(kind, EntryKind::File { .. }) {
            self.ensure_parents(path)?;
        }
        self.entries.insert(path.to_owned(), kind);
        Ok(())
    }

    fn ensure_parents(&mut self, path: &str) -> Result<(), &'static str> {
        let mut offset = 0;
        for component in path.split('/') {
            let parent_end = offset + component.len();
            if parent_end == path.len() {
                break;
            }
            let parent = &path[..parent_end];
            match self.entries.get(parent) {
                Some(EntryKind::Directory) => {}
                Some(EntryKind::File { .. }) => return Err("file-parent"),
                None => {
                    self.entries.insert(parent.to_owned(), EntryKind::Directory);
                }
            }
            offset = parent_end + 1;
        }
        Ok(())
    }

    /// The canonical digest of this tree under `scheme`.
    pub fn digest(&self, scheme: DigestScheme) -> Result<TreeDigest, &'static str> {
        let mut hasher = Sha256::new();
        hasher.update(scheme.domain());
        for (path, kind) in &self.entries {
            hasher.update(path.as_bytes());
            hasher.update([0]);
            match kind {
                EntryKind::Directory => hasher.update([b'd']),
                EntryKind::File {
                    executable,
                    size,
                    content,
                } => {
                    hasher.update([b'f']);
                    hasher.update([u8::from(*executable)]);
                    hasher.update(size.to_be_bytes());
                    match (scheme, content) {
                        (DigestScheme::RegistrySource, ContentHash::Sha256(hash)) => {
                            hasher.update([32]);
                            hasher.update(hash);
                        }
                        (DigestScheme::GitSource, ContentHash::GitObject(object)) => {
                            let length = u8::try_from(object.len()).map_err(|_| "object")?;
                            if length == 0 {
                                return Err("object");
                            }
                            hasher.update([length]);
                            hasher.update(object);
                        }
                        _ => return Err("content-scheme"),
                    }
                }
            }
        }
        let output = hasher.finalize();
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&output);
        Ok(TreeDigest(digest))
    }
}

/// Validate one relative path: nonempty, bounded, no NUL, no empty `.` or
/// `..` component, and not absolute. An empty component also rejects a
/// trailing slash and `//`.
pub fn validate_path(path: &str, max_path_bytes: usize) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("empty");
    }
    if path.len() > max_path_bytes {
        return Err("too-long");
    }
    if path.contains('\0') {
        return Err("nul");
    }
    if path.starts_with('/') {
        return Err("absolute");
    }
    let mut components = path.split('/');
    let first = components.next().ok_or("empty")?;
    if first.is_empty() {
        return Err("absolute");
    }
    for component in std::iter::once(first).chain(components) {
        match component {
            "" => return Err("empty-component"),
            "." => return Err("current"),
            ".." => return Err("parent"),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(source: &[u8], executable: bool) -> EntryKind {
        let mut hasher = Sha256::new();
        hasher.update(source);
        let output = hasher.finalize();
        let mut content = [0_u8; 32];
        content.copy_from_slice(&output);
        EntryKind::File {
            executable,
            size: u64::try_from(source.len()).expect("fixture size"),
            content: ContentHash::Sha256(content),
        }
    }

    fn tree(entries: &[(&str, EntryKind)]) -> SourceTree {
        let mut tree = SourceTree::new();
        for (path, kind) in entries {
            tree.insert_explicit(path, kind.clone(), MAX_PATH_BYTES)
                .expect("fixture path");
        }
        tree
    }

    #[test]
    fn the_digest_is_order_independent_and_complete() {
        let a = ("a.rs", file(b"alpha", false));
        let b = ("dir/b.rs", file(b"beta", true));
        let first = tree(&[a.clone(), b.clone()]);
        let second = tree(&[b.clone(), a.clone()]);
        assert_eq!(first.len(), second.len());
        let digest = first.digest(DigestScheme::RegistrySource).expect("digest");
        assert_eq!(
            digest.to_hex().len(),
            64,
            "a digest renders as 64 hex characters"
        );
        assert_eq!(
            digest,
            second.digest(DigestScheme::RegistrySource).expect("digest")
        );
        assert_eq!(TreeDigest::from_hex(&digest.to_hex()), Some(digest));

        for changed in [
            tree(&[("a.rs", file(b"alphA", false)), b.clone()]),
            tree(&[("a.rs", file(b"alpha", true)), b.clone()]),
            tree(&[("a.rs", file(b"alpha!", false)), b.clone()]),
            tree(&[
                ("a.rs", file(b"alpha", false)),
                ("dir/c.rs", file(b"beta", true)),
            ]),
            tree(&[
                ("a.rs", file(b"alpha", false)),
                ("dir", EntryKind::Directory),
            ]),
        ] {
            assert_ne!(
                digest,
                changed
                    .digest(DigestScheme::RegistrySource)
                    .expect("digest")
            );
        }
    }

    #[test]
    fn the_scheme_is_part_of_the_digest_domain() {
        let mut tree = SourceTree::new();
        tree.insert_explicit(
            "a",
            EntryKind::File {
                executable: false,
                size: 1,
                content: ContentHash::Sha256([7; 32]),
            },
            MAX_PATH_BYTES,
        )
        .expect("file");
        assert_eq!(tree.digest(DigestScheme::GitSource), Err("content-scheme"));
        let mut git = SourceTree::new();
        git.insert_explicit(
            "a",
            EntryKind::File {
                executable: false,
                size: 1,
                content: ContentHash::GitObject(vec![7; 20]),
            },
            MAX_PATH_BYTES,
        )
        .expect("file");
        assert_eq!(
            git.digest(DigestScheme::RegistrySource),
            Err("content-scheme")
        );
        assert!(git.digest(DigestScheme::GitSource).is_ok());
    }

    #[test]
    fn parents_are_implied_and_conflicts_refuse() {
        let mut tree = SourceTree::new();
        tree.insert_explicit("a/b/c", file(b"x", false), MAX_PATH_BYTES)
            .expect("file");
        assert_eq!(tree.get("a"), Some(&EntryKind::Directory));
        assert_eq!(tree.get("a/b"), Some(&EntryKind::Directory));
        assert_eq!(
            tree.insert_explicit("a/b/c", file(b"x", false), MAX_PATH_BYTES),
            Err("duplicate")
        );
        assert_eq!(
            tree.insert_explicit("a", EntryKind::Directory, MAX_PATH_BYTES),
            Err("duplicate")
        );
        let mut conflict = SourceTree::new();
        conflict
            .insert_explicit("a", file(b"x", false), MAX_PATH_BYTES)
            .expect("file");
        assert_eq!(
            conflict.insert_explicit("a/b", file(b"y", false), MAX_PATH_BYTES),
            Err("file-parent")
        );
    }

    #[test]
    fn the_path_grammar_is_strict() {
        for (path, token) in [
            ("", "empty"),
            ("/abs", "absolute"),
            ("a//b", "empty-component"),
            ("a/", "empty-component"),
            ("./a", "current"),
            ("a/./b", "current"),
            ("a/../b", "parent"),
            ("..", "parent"),
            ("a\0b", "nul"),
        ] {
            assert_eq!(validate_path(path, MAX_PATH_BYTES), Err(token), "{path:?}");
        }
        let long = "x".repeat(MAX_PATH_BYTES + 1);
        assert_eq!(validate_path(&long, MAX_PATH_BYTES), Err("too-long"));
        assert_eq!(validate_path("a/very-long", 4), Err("too-long"));
        assert!(validate_path("cfg-if-1.0.0/src/lib.rs", MAX_PATH_BYTES).is_ok());
    }

    #[test]
    fn the_content_hash_is_sha256_of_the_bytes() {
        // The canonical digest must bind exactly the bytes read, so a same-size
        // edit with the original mtime restored still changes it.
        let original = tree(&[("src/lib.rs", file(b"fn a() {}", false))]);
        let edited = tree(&[("src/lib.rs", file(b"fn b() {}", false))]);
        assert_ne!(
            original
                .digest(DigestScheme::RegistrySource)
                .expect("digest"),
            edited.digest(DigestScheme::RegistrySource).expect("digest")
        );
        assert_eq!(
            file(b"fn a() {}", false),
            EntryKind::File {
                executable: false,
                size: 9,
                content: ContentHash::Sha256({
                    let mut hasher = Sha256::new();
                    hasher.update(b"fn a() {}");
                    let output = hasher.finalize();
                    let mut bytes = [0_u8; 32];
                    bytes.copy_from_slice(&output);
                    bytes
                }),
            }
        );
    }
}
