//! Bounded reader for Cargo c980f4866 `EncodedDepInfo` version 1.
//!
//! Decoding retains Cargo's package/build-root path tag and optional checksum.
//! Conversion needs an explicit mapping supplied by the accepted evidence owner;
//! it does not resolve paths, consult the filesystem, or infer a unit graph.

use super::{Association, InputPath, LocalInput, Refusal, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

/// Cargo's encoded dep-info is read under the registered file-read ceiling.
pub const MAX_ENCODED_BYTES: usize = super::Limits::NUCLEUS.file_bytes;
/// Each file and environment reference consumes at least one registered association.
pub const MAX_REFERENCES: usize = super::Limits::NUCLEUS.associations;
/// Strings use the registered path/value ceiling.
pub const MAX_STRING_BYTES: usize = super::Limits::NUCLEUS.path_bytes;

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

/// One exact mapping from a Cargo reference to an owner-registered input.
/// `association` is owner supplied and must name this same input and consuming
/// anchor. Generated mappings also need their registered producer unit.
#[derive(Clone, Debug)]
pub struct RegisteredFile {
    pub reference: FileReference,
    pub input: InputPath,
    pub association: Association,
}

/// Construct this context only inside the accepted `InputEvidenceOwner` after
/// authenticating the unit, anchor and every input association. Task labels are
/// not authority. The runner checks the returned unit against `UnitEvidence.id`.
pub struct TrustedContext<'a> {
    pub unit: &'a str,
    pub anchor: &'a str,
    pub registered_units: &'a BTreeSet<String>,
    pub files: &'a [RegisteredFile],
}

/// The smallest conversion result that an evidence owner can place in one
/// `UnitEvidence`; both the semantic local and explicit owner associations are
/// retained for the registered runner's existing checks.
#[derive(Debug)]
pub struct RegisteredEvidence {
    pub unit: String,
    pub local: LocalInput,
    pub associations: Vec<Association>,
}

#[cfg(test)]
pub(super) const UPSTREAM_V1: &[u8] = &[
    1, 0, 0, 0, 255, 1, 3, 0, 0, 0, 0, 10, 0, 0, 0, b's', b'r', b'c', b'/', b'l', b'i', b'b', b'.',
    b'r', b's', 0, 1, 12, 0, 0, 0, b'b', b'u', b'i', b'l', b'd', b'/', b'o', b'u', b't', b'.',
    b'r', b's', 0, 0, 10, 0, 0, 0, b'C', b'a', b'r', b'g', b'o', b'.', b't', b'o', b'm', b'l', 0,
    5, 0, 0, 0, 6, 0, 0, 0, b'A', b'B', b'S', b'E', b'N', b'T', 0, 5, 0, 0, 0, b'E', b'M', b'P',
    b'T', b'Y', 1, 0, 0, 0, 0, 3, 0, 0, 0, b'S', b'E', b'T', 1, 3, 0, 0, 0, b'y', b'e', b's', 5, 0,
    0, 0, b'K', b'N', b'O', b'W', b'N', 1, 17, 0, 0, 0, b'v', b'a', b'l', b'u', b'e', b' ', b'w',
    b'i', b't', b'h', b' ', b's', b'p', b'a', b'c', b'e', b's', 14, 0, 0, 0, b'C', b'A', b'R',
    b'G', b'O', b'_', b'P', b'K', b'G', b'_', b'N', b'A', b'M', b'E', 1, 14, 0, 0, 0, b's', b'y',
    b'n', b't', b'h', b'e', b't', b'i', b'c', b'-', b'c', b'o', b'r', b'e',
];

impl DecodedDepInfo {
    pub fn into_registered(self, context: TrustedContext<'_>) -> Result<RegisteredEvidence> {
        if context.unit.is_empty()
            || context.unit.len() > MAX_STRING_BYTES
            || context.unit.as_bytes().contains(&0)
            || !context.registered_units.contains(context.unit)
            || context.files.len() > MAX_REFERENCES
            || context.files.len() != self.files.len()
        {
            return Err(Refusal::MissingAuthority);
        }
        super::relative(context.anchor, super::Limits::NUCLEUS)?;
        if self.files.is_empty() {
            return Err(Refusal::Unsupported);
        }

        let mut paths = Vec::with_capacity(self.files.len());
        let mut associations = Vec::with_capacity(self.files.len());
        let mut mappings = BTreeMap::new();
        for mapping in context.files {
            if mappings
                .insert(mapping.reference.clone(), mapping)
                .is_some()
            {
                return Err(Refusal::MissingAuthority);
            }
        }
        let mut used_references = BTreeSet::new();
        let mut used_inputs = BTreeSet::new();
        for file in self.files {
            if file.checksum.is_some() {
                // Existing LocalInput::DepInfo has no checksum semantics. Do not
                // downgrade checksum freshness to the runner's mtime evidence.
                return Err(Refusal::Unsupported);
            }
            validate_relative(&file.reference.path)?;
            if !used_references.insert(file.reference.clone()) {
                return Err(Refusal::MissingAuthority);
            }
            let Some(mapping) = mappings.get(&file.reference) else {
                return Err(Refusal::MissingAuthority);
            };
            super::relative(&mapping.input.path, super::Limits::NUCLEUS)?;
            if mapping.association.input != mapping.input
                || !mapping.association.anchors.contains(context.anchor)
                || !used_inputs.insert(mapping.input.clone())
            {
                return Err(Refusal::MissingAuthority);
            }
            if mapping.input.root == super::InputRoot::Generated
                && !matches!(
                    mapping.association.producer.as_deref(),
                    Some(producer) if context.registered_units.contains(producer)
                )
            {
                return Err(Refusal::MissingAuthority);
            }
            paths.push(mapping.input.clone());
            associations.push(mapping.association.clone());
        }
        if used_references.len() != mappings.len() {
            return Err(Refusal::MissingAuthority);
        }
        for (key, value) in &self.environment {
            if key.is_empty()
                || key.len() > MAX_STRING_BYTES
                || key.contains(['=', '\0'])
                || value.as_ref().is_some_and(|value| {
                    value.len() > MAX_STRING_BYTES || value.as_bytes().contains(&0)
                })
            {
                return Err(Refusal::Unsupported);
            }
        }
        Ok(RegisteredEvidence {
            unit: context.unit.to_owned(),
            local: LocalInput::DepInfo {
                unit: context.unit.to_owned(),
                anchor: context.anchor.to_owned(),
                files: paths,
                environment: self.environment,
            },
            associations,
        })
    }
}

/// Decode the literal byte slice of Cargo c980f4866 `EncodedDepInfo` v1.
/// Unknown versions and checksum records are decoded faithfully; conversion to
/// current registered evidence refuses checksums because that model cannot
/// preserve their freshness rule.
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

fn validate_relative(value: &str) -> Result<()> {
    super::relative(value, super::Limits::NUCLEUS)?;
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Refusal::Unsupported);
    }
    Ok(())
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
    // env ABSENT=None, EMPTY=Some(""), SET=Some("yes"), plus the owner's
    // existing KNOWN and CARGO_PKG_NAME identity controls.

    fn context<'a>(units: &'a BTreeSet<String>, files: &'a [RegisteredFile]) -> TrustedContext<'a> {
        TrustedContext {
            unit: "source-unit",
            anchor: "dep-info",
            registered_units: units,
            files,
        }
    }
    fn mapping(reference: FileReference, input: InputPath) -> RegisteredFile {
        RegisteredFile {
            association: Association {
                input: input.clone(),
                anchors: BTreeSet::from(["dep-info".into()]),
                producer: (input.root == super::super::InputRoot::Generated)
                    .then(|| "script-unit".into()),
                native_receipt: None,
            },
            reference,
            input,
        }
    }
    fn units() -> BTreeSet<String> {
        BTreeSet::from(["source-unit".into(), "script-unit".into()])
    }

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
    fn refuses_duplicate_environment_keys_and_preserves_checksum_for_refusal() {
        // One package file with a checksum, followed by an empty environment.
        let mut bytes = vec![1, 0, 0, 0, 255, 1, 1, 0, 0, 0, 0];
        bytes.extend_from_slice(&[1, 0, 0, 0, b'x', 1]);
        bytes.extend_from_slice(&7u64.to_le_bytes());
        bytes.extend_from_slice(&[3, 0, 0, 0, b's', b'u', b'm']);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let parsed = decode(&bytes).unwrap();
        assert_eq!(parsed.files[0].checksum.as_ref().unwrap().file_size, 7);
        assert_eq!(parsed.files[0].checksum.as_ref().unwrap().value, "sum");
        let input = InputPath {
            root: super::super::InputRoot::Source,
            path: "x".into(),
        };
        let mapping = mapping(parsed.files[0].reference.clone(), input);
        let mappings = [mapping];
        let registered_units = units();
        assert_eq!(
            parsed
                .into_registered(context(&registered_units, &mappings))
                .unwrap_err(),
            Refusal::Unsupported
        );

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

    #[test]
    fn conversion_requires_exact_associations_and_refuses_escape_or_absolute_paths() {
        let parsed = decode(UPSTREAM_V1).unwrap();
        let units = units();
        let package = mapping(
            parsed.files[0].reference.clone(),
            InputPath {
                root: super::super::InputRoot::Source,
                path: "src/lib.rs".into(),
            },
        );
        let generated = mapping(
            parsed.files[1].reference.clone(),
            InputPath {
                root: super::super::InputRoot::Generated,
                path: "generated.rs".into(),
            },
        );
        let manifest = mapping(
            parsed.files[2].reference.clone(),
            InputPath {
                root: super::super::InputRoot::Source,
                path: "Cargo.toml".into(),
            },
        );
        let mappings = [package.clone(), generated.clone(), manifest.clone()];
        let evidence = decode(UPSTREAM_V1)
            .unwrap()
            .into_registered(context(&units, &mappings))
            .unwrap();
        assert_eq!(evidence.unit, "source-unit");
        assert_eq!(evidence.associations.len(), 3);
        assert!(matches!(
            evidence.local,
            LocalInput::DepInfo { unit, files, environment, .. }
                if unit == "source-unit"
                    && files.len() == 3
                    && environment.get("EMPTY") == Some(&Some(String::new()))
        ));

        assert_eq!(
            decode(UPSTREAM_V1)
                .unwrap()
                .into_registered(context(&units, &mappings[..1]))
                .unwrap_err(),
            Refusal::MissingAuthority
        );
        let ambiguous = [package.clone(), package.clone(), manifest.clone()];
        assert_eq!(
            decode(UPSTREAM_V1)
                .unwrap()
                .into_registered(context(&units, &ambiguous))
                .unwrap_err(),
            Refusal::MissingAuthority
        );
        let mut no_producer = generated.clone();
        no_producer.association.producer = None;
        let missing_producer = [package.clone(), no_producer, manifest.clone()];
        assert_eq!(
            decode(UPSTREAM_V1)
                .unwrap()
                .into_registered(context(&units, &missing_producer))
                .unwrap_err(),
            Refusal::MissingAuthority
        );
        let mut escaped = decode(UPSTREAM_V1).unwrap();
        escaped.files[0].reference.path = "../outside.rs".into();
        let escape_mappings = [
            mapping(
                escaped.files[0].reference.clone(),
                InputPath {
                    root: super::super::InputRoot::Source,
                    path: "src.rs".into(),
                },
            ),
            generated.clone(),
            manifest.clone(),
        ];
        assert_eq!(
            escaped
                .into_registered(context(&units, &escape_mappings))
                .unwrap_err(),
            Refusal::Unsupported
        );
        let mut absolute = decode(UPSTREAM_V1).unwrap();
        absolute.files[1].reference.path = "/outside.rs".into();
        let absolute_mappings = [
            package.clone(),
            mapping(
                absolute.files[1].reference.clone(),
                InputPath {
                    root: super::super::InputRoot::Generated,
                    path: "generated.rs".into(),
                },
            ),
            manifest.clone(),
        ];
        assert_eq!(
            absolute
                .into_registered(context(&units, &absolute_mappings))
                .unwrap_err(),
            Refusal::Unsupported
        );
        let mut escaping_mapping = mappings[0].clone();
        escaping_mapping.input.path = "../outside.rs".into();
        escaping_mapping.association.input = escaping_mapping.input.clone();
        let unsafe_mappings = [escaping_mapping, generated, manifest];
        assert_eq!(
            decode(UPSTREAM_V1)
                .unwrap()
                .into_registered(context(&units, &unsafe_mappings))
                .unwrap_err(),
            Refusal::Unsupported
        );
    }
}
