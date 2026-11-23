//! Parse the Linux `posix_acl_xattr` encoding of a POSIX access ACL.
//!
//! The encoding is a little-endian version word followed by 8-byte entries of
//! `{tag: u16, perm: u16, id: u32}`. Parsing fails closed on any structure it
//! does not recognise, so a caller can treat an error as a refusal.

/// `ACL_READ`, `ACL_WRITE` and `ACL_EXECUTE` are the three permission bits.
pub(super) const WRITE: u16 = 0x2;
const ALL: u16 = 0x7;

const VERSION: u32 = 2;
const ENTRY_BYTES: usize = 8;

const USER_OBJ: u16 = 0x01;
const USER: u16 = 0x02;
const GROUP_OBJ: u16 = 0x04;
const GROUP: u16 = 0x08;
const MASK: u16 = 0x10;
const OTHER: u16 = 0x20;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Malformed(pub(super) &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    UserObj,
    User,
    GroupObj,
    Group,
    Mask,
    Other,
}

struct Entry {
    tag: Tag,
    perm: u16,
    id: u32,
}

fn tag(value: u16) -> Result<Tag, Malformed> {
    Ok(match value {
        USER_OBJ => Tag::UserObj,
        USER => Tag::User,
        GROUP_OBJ => Tag::GroupObj,
        GROUP => Tag::Group,
        MASK => Tag::Mask,
        OTHER => Tag::Other,
        _ => return Err(Malformed("unknown entry tag")),
    })
}

fn entries(acl: &[u8]) -> Result<Vec<Entry>, Malformed> {
    let [v0, v1, v2, v3, body @ ..] = acl else {
        return Err(Malformed("truncated header"));
    };
    if u32::from_le_bytes([*v0, *v1, *v2, *v3]) != VERSION {
        return Err(Malformed("unknown ACL version"));
    }
    let chunks = body.chunks_exact(ENTRY_BYTES);
    if !chunks.remainder().is_empty() {
        return Err(Malformed("truncated entry"));
    }
    chunks
        .map(|chunk| {
            let entry: [u8; ENTRY_BYTES] =
                chunk.try_into().map_err(|_| Malformed("truncated entry"))?;
            let perm = u16::from_le_bytes([entry[2], entry[3]]);
            if perm & !ALL != 0 {
                return Err(Malformed("permission outside rwx"));
            }
            Ok(Entry {
                tag: tag(u16::from_le_bytes([entry[0], entry[1]]))?,
                perm,
                id: u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]),
            })
        })
        .collect()
}

/// The union of effective permissions the access ACL grants to principals other
/// than the owner and the `trusted` user IDs.
///
/// `ACL_MASK` limits named users, the owning group and named groups. The owner
/// and `ACL_OTHER` are not masked. An ACL without a mask entry is treated as
/// fully masked, which matches the kernel's view of a mode-only ACL.
pub(super) fn untrusted_permissions(acl: &[u8], trusted: &[u32]) -> Result<u16, Malformed> {
    let entries = entries(acl)?;
    let count = |wanted: Tag| entries.iter().filter(|entry| entry.tag == wanted).count();
    if count(Tag::UserObj) != 1 || count(Tag::GroupObj) != 1 || count(Tag::Other) != 1 {
        return Err(Malformed("owner, group or other entry is not unique"));
    }
    if count(Tag::Mask) > 1 {
        return Err(Malformed("more than one mask entry"));
    }
    let mask_entry = entries.iter().find(|entry| entry.tag == Tag::Mask);
    let named = entries
        .iter()
        .any(|entry| matches!(entry.tag, Tag::User | Tag::Group));
    if named && mask_entry.is_none() {
        return Err(Malformed("named entry without a mask"));
    }
    let mask = mask_entry.map_or(ALL, |entry| entry.perm);

    let mut granted = 0;
    for entry in &entries {
        match entry.tag {
            Tag::UserObj | Tag::Mask => {}
            Tag::User if trusted.contains(&entry.id) => {}
            Tag::User | Tag::GroupObj | Tag::Group => granted |= entry.perm & mask,
            Tag::Other => granted |= entry.perm,
        }
    }
    Ok(granted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNDEFINED: u32 = u32::MAX;
    const ROOT: u32 = 0;
    const OWNER: u32 = 1000;
    const NOBODY: u32 = 65534;
    const TRUSTED: [u32; 2] = [ROOT, OWNER];

    fn acl(entries: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut bytes = VERSION.to_le_bytes().to_vec();
        for &(tag, perm, id) in entries {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&perm.to_le_bytes());
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        bytes
    }

    /// A read-and-traverse ancestor such as `/home`, with one named reader.
    fn reader_ancestor() -> Vec<u8> {
        acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (USER, 5, NOBODY),
            (GROUP_OBJ, 5, UNDEFINED),
            (MASK, 5, UNDEFINED),
            (OTHER, 5, UNDEFINED),
        ])
    }

    #[test]
    fn read_and_traverse_ancestor_grants_no_write() {
        let granted = untrusted_permissions(&reader_ancestor(), &TRUSTED).unwrap();
        assert_eq!(granted, 5);
        assert_eq!(granted & WRITE, 0);
    }

    #[test]
    fn write_for_another_principal_is_granted_on_an_ancestor() {
        let other_write = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (GROUP_OBJ, 5, UNDEFINED),
            (OTHER, 7, UNDEFINED),
        ]);
        assert_ne!(
            untrusted_permissions(&other_write, &TRUSTED).unwrap() & WRITE,
            0
        );

        let named_write = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (USER, 6, NOBODY),
            (GROUP_OBJ, 5, UNDEFINED),
            (MASK, 7, UNDEFINED),
            (OTHER, 5, UNDEFINED),
        ]);
        assert_ne!(
            untrusted_permissions(&named_write, &TRUSTED).unwrap() & WRITE,
            0
        );
    }

    #[test]
    fn mask_removes_write_from_a_named_entry() {
        let masked = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (USER, 7, NOBODY),
            (GROUP_OBJ, 5, UNDEFINED),
            (MASK, 5, UNDEFINED),
            (OTHER, 0, UNDEFINED),
        ]);
        assert_eq!(untrusted_permissions(&masked, &TRUSTED).unwrap(), 5);
    }

    #[test]
    fn trusted_entries_do_not_count_as_untrusted() {
        let trusted = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (USER, 7, ROOT),
            (USER, 7, OWNER),
            (GROUP_OBJ, 0, UNDEFINED),
            (MASK, 7, UNDEFINED),
            (OTHER, 0, UNDEFINED),
        ]);
        assert_eq!(untrusted_permissions(&trusted, &TRUSTED).unwrap(), 0);
    }

    #[test]
    fn private_leaf_refuses_any_untrusted_permission() {
        let leaf = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (USER, 6, NOBODY),
            (GROUP_OBJ, 0, UNDEFINED),
            (MASK, 6, UNDEFINED),
            (OTHER, 0, UNDEFINED),
        ]);
        assert_ne!(untrusted_permissions(&leaf, &TRUSTED).unwrap(), 0);

        let fully_masked = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (USER, 7, NOBODY),
            (GROUP_OBJ, 0, UNDEFINED),
            (MASK, 0, UNDEFINED),
            (OTHER, 0, UNDEFINED),
        ]);
        assert_eq!(untrusted_permissions(&fully_masked, &TRUSTED).unwrap(), 0);
    }

    #[test]
    fn malformed_acls_fail_closed() {
        let good = reader_ancestor();
        let mut wrong_version = good.clone();
        wrong_version[0] = 1;
        assert!(untrusted_permissions(&wrong_version, &TRUSTED).is_err());

        let truncated = &good[..good.len() - 3];
        assert!(untrusted_permissions(truncated, &TRUSTED).is_err());
        assert!(untrusted_permissions(&good[..2], &TRUSTED).is_err());

        let unknown_tag = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (0x40, 7, 0),
            (GROUP_OBJ, 5, UNDEFINED),
            (OTHER, 5, UNDEFINED),
        ]);
        assert!(untrusted_permissions(&unknown_tag, &TRUSTED).is_err());

        let wide_perm = acl(&[
            (USER_OBJ, 8, UNDEFINED),
            (GROUP_OBJ, 5, UNDEFINED),
            (OTHER, 5, UNDEFINED),
        ]);
        assert!(untrusted_permissions(&wide_perm, &TRUSTED).is_err());

        let named_without_mask = acl(&[
            (USER_OBJ, 7, UNDEFINED),
            (USER, 7, NOBODY),
            (GROUP_OBJ, 5, UNDEFINED),
            (OTHER, 5, UNDEFINED),
        ]);
        assert!(untrusted_permissions(&named_without_mask, &TRUSTED).is_err());

        let missing_other = acl(&[(USER_OBJ, 7, UNDEFINED), (GROUP_OBJ, 5, UNDEFINED)]);
        assert!(untrusted_permissions(&missing_other, &TRUSTED).is_err());
    }
}
