//! Bounded reader for Cargo c980f4866 `EncodedDepInfo` version 1.
//!
//! The format (marker `01 00 00 00 ff`, version 1) is identical in the
//! pinned source of Cargo 1.95.0 (f2d3ce0bd), 1.96.1 (356927216), 1.97.1
//! (c980f4866), 1.98.1 (797e8a9bc) and 1.99.1 (5f94df478), and decoded every
//! fixture unit each wrote (oxide#091). Any other marker or version refuses.
//!
//! Decoding retains Cargo's package/build-root path tag, optional checksum and
//! environment presence. It does not resolve paths, consult the filesystem or
//! infer a unit graph. Kept from the retired registered route as input for the
//! cross-checkout freshness gate (W-8); no current command calls it.

use std::collections::{BTreeMap, BTreeSet};

/// Encoded dep-info larger than this refuses before decoding.
pub const MAX_ENCODED_BYTES: usize = 16 * 1024 * 1024;
/// Ceiling on file references and on environment entries.
pub const MAX_REFERENCES: usize = 32768;
/// Ceiling on each path, key or value string.
pub const MAX_STRING_BYTES: usize = 2048;

/// Fail-closed decoding refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// Unknown marker, version or tag, invalid UTF-8, duplicate or trailing data.
    Unsupported,
    /// A length, count or total size exceeds its ceiling or the remaining input.
    Budget,
}

pub type Result<T> = std::result::Result<T, Refusal>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PathTag {
    PackageRootRelative,
    BuildRootRelative,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FileReference {
    pub tag: PathTag,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checksum {
    pub file_size: u64,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct File {
    pub reference: FileReference,
    pub checksum: Option<Checksum>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedDepInfo {
    pub files: Vec<File>,
    /// `None` means absent; `Some("")` means explicitly set to the empty value.
    pub environment: BTreeMap<String, Option<String>>,
}

/// Decode the literal byte slice of Cargo c980f4866 `EncodedDepInfo` v1.
/// Checksum records are decoded faithfully; unknown versions, tags and
/// trailing bytes refuse.
pub fn decode(bytes: &[u8]) -> Result<DecodedDepInfo> {
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(Refusal::Budget);
    }
    let mut reader = Reader { bytes, offset: 0 };
    if reader.take(5)? != [1, 0, 0, 0, u8::MAX] || reader.u8()? != 1 {
        return Err(Refusal::Unsupported);
    }

    let file_count = reader.count(MAX_REFERENCES, 6)?;
    let mut files = Vec::with_capacity(file_count);
    let mut seen_files = BTreeSet::new();
    for _ in 0..file_count {
        let tag = match reader.u8()? {
            0 => PathTag::PackageRootRelative,
            1 => PathTag::BuildRootRelative,
            _ => return Err(Refusal::Unsupported),
        };
        let path = reader.string(MAX_STRING_BYTES)?;
        let reference = FileReference { tag, path };
        if !seen_files.insert(reference.clone()) {
            return Err(Refusal::Unsupported);
        }
        let checksum = match reader.boolean()? {
            false => None,
            true => Some(Checksum {
                file_size: reader.u64()?,
                value: reader.string(MAX_STRING_BYTES)?,
            }),
        };
        files.push(File {
            reference,
            checksum,
        });
    }

    let env_count = reader.count(MAX_REFERENCES, 5)?;
    let mut environment = BTreeMap::new();
    for _ in 0..env_count {
        let key = reader.string(MAX_STRING_BYTES)?;
        let value = match reader.u8()? {
            0 => None,
            1 => Some(reader.string(MAX_STRING_BYTES)?),
            _ => return Err(Refusal::Unsupported),
        };
        if environment.insert(key, value).is_some() {
            return Err(Refusal::Unsupported);
        }
    }
    if reader.offset != bytes.len() {
        return Err(Refusal::Unsupported);
    }
    Ok(DecodedDepInfo { files, environment })
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    fn take(&mut self, len: usize) -> Result<&[u8]> {
        let end = self.offset.checked_add(len).ok_or(Refusal::Budget)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(Refusal::Unsupported)?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn boolean(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Refusal::Unsupported),
        }
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| Refusal::Unsupported)?,
        ))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| Refusal::Unsupported)?,
        ))
    }
    fn count(&mut self, maximum: usize, minimum_item_bytes: usize) -> Result<usize> {
        let count = usize::try_from(self.u32()?).map_err(|_| Refusal::Budget)?;
        if count > maximum || count > self.remaining() / minimum_item_bytes {
            return Err(Refusal::Budget);
        }
        Ok(count)
    }
    fn string(&mut self, maximum: usize) -> Result<String> {
        let len = usize::try_from(self.u32()?).map_err(|_| Refusal::Budget)?;
        if len > maximum || len > self.remaining() {
            return Err(Refusal::Budget);
        }
        let bytes = self.take(len)?;
        let value = std::str::from_utf8(bytes).map_err(|_| Refusal::Unsupported)?;
        if value.as_bytes().contains(&0) {
            return Err(Refusal::Unsupported);
        }
        Ok(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hand-annotated from Cargo EncodedDepInfo::serialize at c980f4866:
    // marker 1/ff, version 1; package `src/lib.rs`, build-root `build/out.rs`,
    // and package `Cargo.toml`;
    // env ABSENT=None, EMPTY=Some(""), SET=Some("yes"), plus KNOWN and
    // CARGO_PKG_NAME controls.
    const UPSTREAM_V1: &[u8] = &[
        1, 0, 0, 0, 255, 1, 3, 0, 0, 0, 0, 10, 0, 0, 0, b's', b'r', b'c', b'/', b'l', b'i', b'b',
        b'.', b'r', b's', 0, 1, 12, 0, 0, 0, b'b', b'u', b'i', b'l', b'd', b'/', b'o', b'u', b't',
        b'.', b'r', b's', 0, 0, 10, 0, 0, 0, b'C', b'a', b'r', b'g', b'o', b'.', b't', b'o', b'm',
        b'l', 0, 5, 0, 0, 0, 6, 0, 0, 0, b'A', b'B', b'S', b'E', b'N', b'T', 0, 5, 0, 0, 0, b'E',
        b'M', b'P', b'T', b'Y', 1, 0, 0, 0, 0, 3, 0, 0, 0, b'S', b'E', b'T', 1, 3, 0, 0, 0, b'y',
        b'e', b's', 5, 0, 0, 0, b'K', b'N', b'O', b'W', b'N', 1, 17, 0, 0, 0, b'v', b'a', b'l',
        b'u', b'e', b' ', b'w', b'i', b't', b'h', b' ', b's', b'p', b'a', b'c', b'e', b's', 14, 0,
        0, 0, b'C', b'A', b'R', b'G', b'O', b'_', b'P', b'K', b'G', b'_', b'N', b'A', b'M', b'E',
        1, 14, 0, 0, 0, b's', b'y', b'n', b't', b'h', b'e', b't', b'i', b'c', b'-', b'c', b'o',
        b'r', b'e',
    ];

    #[test]
    fn hand_annotated_upstream_bytes_preserve_tags_and_environment_presence() {
        let parsed = decode(UPSTREAM_V1).unwrap();
        assert_eq!(parsed.files.len(), 3);
        assert_eq!(parsed.files[0].reference.tag, PathTag::PackageRootRelative);
        assert_eq!(parsed.files[0].reference.path, "src/lib.rs");
        assert_eq!(parsed.files[1].reference.tag, PathTag::BuildRootRelative);
        assert_eq!(parsed.files[1].reference.path, "build/out.rs");
        assert_eq!(parsed.environment.get("ABSENT"), Some(&None));
        assert_eq!(parsed.environment.get("EMPTY"), Some(&Some(String::new())));
        assert_eq!(parsed.environment.get("SET"), Some(&Some("yes".into())));
    }

    #[test]
    fn refuses_truncation_version_marker_tag_bool_trailing_and_oversize() {
        for end in 0..UPSTREAM_V1.len() {
            assert!(decode(&UPSTREAM_V1[..end]).is_err(), "prefix {end}");
        }
        let mut changed = UPSTREAM_V1.to_vec();
        changed[5] = 2;
        assert_eq!(decode(&changed), Err(Refusal::Unsupported));
        let mut changed = UPSTREAM_V1.to_vec();
        changed[4] = 254;
        assert_eq!(decode(&changed), Err(Refusal::Unsupported));
        let mut changed = UPSTREAM_V1.to_vec();
        changed[10] = 2;
        assert_eq!(decode(&changed), Err(Refusal::Unsupported));
        let mut changed = UPSTREAM_V1.to_vec();
        changed[15] = 255;
        assert_eq!(decode(&changed), Err(Refusal::Unsupported));
        let mut changed = UPSTREAM_V1.to_vec();
        changed[25] = 2;
        assert_eq!(decode(&changed), Err(Refusal::Unsupported));
        let mut changed = UPSTREAM_V1.to_vec();
        changed[74] = 2;
        assert_eq!(decode(&changed), Err(Refusal::Unsupported));
        let mut changed = UPSTREAM_V1.to_vec();
        changed.push(0);
        assert_eq!(decode(&changed), Err(Refusal::Unsupported));
        let mut count = UPSTREAM_V1[..6].to_vec();
        count.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&count), Err(Refusal::Budget));
        let mut env_count = UPSTREAM_V1[..60].to_vec();
        env_count.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&env_count), Err(Refusal::Budget));
        let mut len = UPSTREAM_V1[..10].to_vec();
        len.extend_from_slice(&[0, 1, 8, 0, 0, 0]);
        assert_eq!(decode(&len), Err(Refusal::Budget));
        let oversized = vec![0; MAX_ENCODED_BYTES + 1];
        assert_eq!(decode(&oversized), Err(Refusal::Budget));
    }

    #[test]
    fn refuses_duplicate_keys_and_files_and_preserves_checksums() {
        // One package file with a checksum, followed by an empty environment.
        let mut bytes = vec![1, 0, 0, 0, 255, 1, 1, 0, 0, 0, 0];
        bytes.extend_from_slice(&[1, 0, 0, 0, b'x', 1]);
        bytes.extend_from_slice(&7u64.to_le_bytes());
        bytes.extend_from_slice(&[3, 0, 0, 0, b's', b'u', b'm']);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let parsed = decode(&bytes).unwrap();
        assert_eq!(parsed.files[0].checksum.as_ref().unwrap().file_size, 7);
        assert_eq!(parsed.files[0].checksum.as_ref().unwrap().value, "sum");

        // Duplicate keys are invalid even when their values happen to agree.
        let mut duplicate = vec![1, 0, 0, 0, 255, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, b'x', 0];
        duplicate.extend_from_slice(&[1, 0, 0, 0]);
        duplicate.extend_from_slice(&[1, 0, 0, 0, b'K', 0]);
        duplicate.extend_from_slice(&[1, 0, 0, 0, b'K', 0]);
        assert_eq!(decode(&duplicate), Err(Refusal::Unsupported));

        let mut duplicate_file = vec![1, 0, 0, 0, 255, 1, 2, 0, 0, 0];
        duplicate_file.extend_from_slice(&[0, 1, 0, 0, 0, b'x', 0]);
        duplicate_file.extend_from_slice(&[0, 1, 0, 0, 0, b'x', 0]);
        duplicate_file.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(decode(&duplicate_file), Err(Refusal::Unsupported));
    }
}
