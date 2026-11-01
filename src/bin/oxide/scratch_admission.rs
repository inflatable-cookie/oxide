//! Pure scratch-pool admission. No caller is wired until S2.
use crate::config::{PathRefusal, Source};
use serde::Deserialize;
use std::path::PathBuf;

/// Documents have already been read with their file type, owner and size checked.
#[derive(Debug)]
pub(crate) enum Document<'a> {
    Missing,
    Invalid,
    Bytes(&'a [u8]),
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct Binding {
    schema: String,
    volume_uuid: String,
    mount_point: PathBuf,
    scratch_root: PathBuf,
    pool: PathBuf,
    created_ms: u64,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: String,
    pool: PathBuf,
    volume_uuid: String,
    mount_point: PathBuf,
    scratch_root: PathBuf,
    created_ms: u64,
}

#[derive(Debug)]
pub(crate) struct Facts<'a> {
    pub ordinary: Result<(), PathRefusal>,
    /// The canonical ancestor and result of its Git recheck, supplied together.
    pub git_recheck: (PathBuf, Result<(), PathRefusal>),
    pub declaration: Option<PathBuf>,
    pub source: Source,
    pub budget_source: Source,
    pub pool: PathBuf,
    /// Injected metadata facts, including canonicality, ownership, mode and device.
    pub root_safe: bool,
    pub parent_exists: bool,
    pub volume_present: bool,
    /// Includes Binding::open and verify, mount identity and /Volumes parent rules.
    pub volume_verified: bool,
    pub volume_uuid: String,
    pub mount_point: PathBuf,
    pub binding: Document<'a>,
    pub record: Document<'a>,
    pub pool_exists: bool,
    pub incomplete: bool,
    /// Only true after this invocation created the leaf and its binding.
    pub record_write_failed_after_creation: bool,
    pub status: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Refusal {
    Path(PathRefusal),
    Class(&'static str),
    VolumeAbsent {
        uuid: Option<String>,
        mount: Option<PathBuf>,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    Ordinary,
    Admit,
    Refuse(Refusal),
    Unavailable(Refusal),
    /// The caller must verify identity and remove only its own leaf; S1 does no I/O.
    RefuseAndRemoveOwnLeaf(Refusal),
}

/// Decide only from the supplied facts; never read or write the filesystem.
pub(crate) fn decide(f: Facts<'_>) -> Decision {
    let refusal = |reason| {
        if f.status {
            Decision::Unavailable(reason)
        } else {
            Decision::Refuse(reason)
        }
    };
    if f.status && matches!(f.record, Document::Missing) {
        return Decision::Ordinary;
    }
    if !f.status {
        match f.ordinary.clone() {
            Ok(()) => return Decision::Ordinary,
            Err(ref external @ PathRefusal::ExternalVolume { ref canonical, .. })
                if f.declaration.is_some() =>
            {
                // A result from any other ancestor is not authority to lift this refusal.
                if *canonical != f.git_recheck.0 {
                    return Decision::Refuse(Refusal::Path(external.clone()));
                }
                if let Err(e) = f.git_recheck.1 {
                    return Decision::Refuse(Refusal::Path(e));
                }
            }
            Err(e) => return Decision::Refuse(Refusal::Path(e)),
        }
        if f.source == Source::Default {
            return refusal(Refusal::Class("scratch-declaration-default-pool"));
        }
    }
    let record = match f.record {
        Document::Missing => None,
        Document::Bytes(bytes) if bytes.len() <= 4096 => {
            match serde_json::from_slice::<Record>(bytes) {
                Ok(r) if r.schema == "oxide.scratch-pool-record/1" && r.pool == f.pool => Some(r),
                _ => return refusal(Refusal::Class("scratch-record-invalid")),
            }
        }
        _ => return refusal(Refusal::Class("scratch-record-invalid")),
    };
    if !f.volume_present {
        return refusal(Refusal::VolumeAbsent {
            uuid: record.as_ref().map(|r| r.volume_uuid.clone()),
            mount: record.as_ref().map(|r| r.mount_point.clone()),
        });
    }
    if f.status {
        match f.ordinary {
            Err(ref external @ PathRefusal::ExternalVolume { ref canonical, .. }) => {
                if *canonical != f.git_recheck.0 {
                    return refusal(Refusal::Path(external.clone()));
                }
                if let Err(e) = f.git_recheck.1 {
                    return refusal(Refusal::Path(e));
                }
            }
            Err(e) => return refusal(Refusal::Path(e)),
            Ok(()) => return refusal(Refusal::Class("scratch-volume-unverified")),
        }
    }
    let root = if f.status {
        record.as_ref().map(|r| &r.scratch_root)
    } else {
        f.declaration.as_ref()
    };
    let Some(root) = root else {
        return refusal(Refusal::Class("scratch-root-unsafe"));
    };
    if !root.is_absolute() || !f.root_safe {
        return refusal(Refusal::Class("scratch-root-unsafe"));
    }
    if f.pool == *root || !f.pool.starts_with(root) {
        return refusal(Refusal::Class("scratch-path-outside-root"));
    }
    if !f.parent_exists {
        return refusal(Refusal::Class("scratch-parent-missing"));
    }
    if !f.volume_verified || f.pool == f.mount_point || !f.pool.starts_with(&f.mount_point) {
        return refusal(Refusal::Class("scratch-volume-unverified"));
    }
    let binding = match f.binding {
        Document::Missing if f.pool_exists => {
            return refusal(Refusal::Class("scratch-binding-missing"))
        }
        Document::Missing => None,
        Document::Bytes(bytes) if bytes.len() <= 16384 => {
            match serde_json::from_slice::<Binding>(bytes) {
                Ok(b) if b.schema == "oxide.scratch-pool/1" => Some(b),
                _ => return refusal(Refusal::Class("scratch-binding-invalid")),
            }
        }
        _ => return refusal(Refusal::Class("scratch-binding-invalid")),
    };
    if let Some(b) = &binding {
        if b.volume_uuid != f.volume_uuid
            || b.mount_point != f.mount_point
            || b.scratch_root != *root
            || b.pool != f.pool
        {
            return refusal(Refusal::Class("scratch-binding-mismatch"));
        }
    }
    if let Some(r) = &record {
        if r.volume_uuid != f.volume_uuid
            || r.mount_point != f.mount_point
            || r.scratch_root != *root
            || binding.as_ref().is_none_or(|b| {
                r.volume_uuid != b.volume_uuid
                    || r.mount_point != b.mount_point
                    || r.scratch_root != b.scratch_root
                    || r.pool != b.pool
            })
        {
            return refusal(Refusal::Class("scratch-record-mismatch"));
        }
    }
    if f.incomplete {
        return refusal(Refusal::Class("scratch-incomplete"));
    }
    if !f.status && f.budget_source == Source::Default {
        return refusal(Refusal::Class("scratch-budget-required"));
    }
    if f.record_write_failed_after_creation {
        return Decision::RefuseAndRemoveOwnLeaf(Refusal::Class("scratch-record-unwritable"));
    }
    Decision::Admit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const BINDING: &[u8] = br#"{"schema":"oxide.scratch-pool/1","volume_uuid":"uuid","mount_point":"/Volumes/Builds","scratch_root":"/Volumes/Builds/root","pool":"/Volumes/Builds/root/pool","created_ms":1}"#;
    const RECORD: &[u8] = br#"{"schema":"oxide.scratch-pool-record/1","volume_uuid":"uuid","mount_point":"/Volumes/Builds","scratch_root":"/Volumes/Builds/root","pool":"/Volumes/Builds/root/pool","created_ms":2}"#;
    const BAD_BINDING: &[u8] = br#"{"schema":"oxide.scratch-pool/1","volume_uuid":"other","mount_point":"/Volumes/Builds","scratch_root":"/Volumes/Builds/root","pool":"/Volumes/Builds/root/pool","created_ms":1}"#;
    const BAD_RECORD: &[u8] = br#"{"schema":"oxide.scratch-pool-record/1","volume_uuid":"other","mount_point":"/Volumes/Builds","scratch_root":"/Volumes/Builds/root","pool":"/Volumes/Builds/root/pool","created_ms":2}"#;

    fn external() -> PathRefusal {
        PathRefusal::ExternalVolume {
            message: "external message".into(),
            canonical: "/Volumes/Builds/root".into(),
        }
    }
    fn facts() -> Facts<'static> {
        Facts {
            ordinary: Err(external()),
            git_recheck: ("/Volumes/Builds/root".into(), Ok(())),
            declaration: Some("/Volumes/Builds/root".into()),
            source: Source::Environment("OXIDE_POOL"),
            budget_source: Source::File,
            pool: "/Volumes/Builds/root/pool".into(),
            root_safe: true,
            parent_exists: true,
            volume_present: true,
            volume_verified: true,
            volume_uuid: "uuid".into(),
            mount_point: "/Volumes/Builds".into(),
            binding: Document::Bytes(BINDING),
            record: Document::Bytes(RECORD),
            pool_exists: true,
            incomplete: false,
            record_write_failed_after_creation: false,
            status: false,
        }
    }
    fn class(name: &'static str) -> Decision {
        Decision::Refuse(Refusal::Class(name))
    }
    #[test]
    fn refusal_matrix() {
        // Rows 1-4: ordinary precedence and the same-ancestor Git recheck.
        let mut f = facts();
        f.ordinary = Err(PathRefusal::Traversal("traversal".into()));
        assert_eq!(
            decide(f),
            Decision::Refuse(Refusal::Path(PathRefusal::Traversal("traversal".into())))
        );
        let mut f = facts();
        f.ordinary = Err(PathRefusal::Ancestor("ancestor".into()));
        assert_eq!(
            decide(f),
            Decision::Refuse(Refusal::Path(PathRefusal::Ancestor("ancestor".into())))
        );
        let mut f = facts();
        f.ordinary = Err(PathRefusal::GitWorkTree("git".into()));
        assert_eq!(
            decide(f),
            Decision::Refuse(Refusal::Path(PathRefusal::GitWorkTree("git".into())))
        );
        let mut f = facts();
        f.declaration = None;
        assert_eq!(decide(f), Decision::Refuse(Refusal::Path(external())));
        let mut f = facts();
        f.git_recheck.1 = Err(PathRefusal::GitWorkTree("git".into()));
        assert_eq!(
            decide(f),
            Decision::Refuse(Refusal::Path(PathRefusal::GitWorkTree("git".into())))
        );
        let mut f = facts();
        f.git_recheck.0 = "/elsewhere".into();
        assert_eq!(decide(f), Decision::Refuse(Refusal::Path(external())));
        let mut f = facts();
        f.source = Source::Default;
        assert_eq!(decide(f), class("scratch-declaration-default-pool")); // 5
        for _ in 6..=8 {
            let mut f = facts();
            f.root_safe = false;
            assert_eq!(decide(f), class("scratch-root-unsafe"));
        }
        for pool in [
            "/Volumes/Builds/root",
            "/Volumes/Builds/root-other/pool",
            "/Volumes/Builds/else/pool",
        ] {
            let mut f = facts();
            f.pool = pool.into();
            f.record = Document::Missing;
            assert_eq!(decide(f), class("scratch-path-outside-root"));
        } // 9
        let mut f = facts();
        f.parent_exists = false;
        assert_eq!(decide(f), class("scratch-parent-missing")); // 10
        for _ in 11..=12 {
            let mut f = facts();
            f.volume_verified = false;
            assert_eq!(decide(f), class("scratch-volume-unverified"));
        }
        let mut f = facts();
        f.binding = Document::Missing;
        assert_eq!(decide(f), class("scratch-binding-missing")); // 13
        for document in [
            Document::Invalid,
            Document::Bytes(b"{}"),
            Document::Bytes(b"not json"),
        ] {
            let mut f = facts();
            f.binding = document;
            assert_eq!(decide(f), class("scratch-binding-invalid"));
        } // 14
        let mut f = facts();
        f.binding = Document::Bytes(BAD_BINDING);
        assert_eq!(decide(f), class("scratch-binding-mismatch")); // 15
        for document in [
            Document::Invalid,
            Document::Bytes(b"{}"),
            Document::Bytes(b"not json"),
        ] {
            let mut f = facts();
            f.record = document;
            assert_eq!(decide(f), class("scratch-record-invalid"));
        } // 16
        let mut f = facts();
        f.record = Document::Bytes(BAD_RECORD);
        assert_eq!(decide(f), class("scratch-record-mismatch")); // 17
        let mut f = facts();
        f.record_write_failed_after_creation = true;
        assert_eq!(
            decide(f),
            Decision::RefuseAndRemoveOwnLeaf(Refusal::Class("scratch-record-unwritable"))
        ); // 18
        let mut f = facts();
        f.volume_present = false;
        assert_eq!(
            decide(f),
            Decision::Refuse(Refusal::VolumeAbsent {
                uuid: Some("uuid".into()),
                mount: Some("/Volumes/Builds".into())
            })
        );
        let mut f = facts();
        f.volume_present = false;
        f.record = Document::Missing;
        assert_eq!(
            decide(f),
            Decision::Refuse(Refusal::VolumeAbsent {
                uuid: None,
                mount: None
            })
        ); // 19
        let mut f = facts();
        f.incomplete = true;
        assert_eq!(decide(f), class("scratch-incomplete")); // 20
        let mut f = facts();
        f.budget_source = Source::Default;
        assert_eq!(decide(f), class("scratch-budget-required")); // 21
        let mut f = facts();
        f.declaration = Some("relative".into());
        assert_eq!(decide(f), class("scratch-root-unsafe")); // 22
        let mut f = facts();
        f.declaration = None;
        assert_eq!(decide(f), Decision::Refuse(Refusal::Path(external()))); // 23
        let mut f = facts();
        f.status = true;
        f.volume_present = false;
        assert_eq!(
            decide(f),
            Decision::Unavailable(Refusal::VolumeAbsent {
                uuid: Some("uuid".into()),
                mount: Some("/Volumes/Builds".into())
            })
        );
        let mut f = facts();
        f.status = true;
        f.binding = Document::Invalid;
        assert_eq!(
            decide(f),
            Decision::Unavailable(Refusal::Class("scratch-binding-invalid"))
        );
        let mut f = facts();
        f.status = true;
        f.record = Document::Missing;
        assert_eq!(decide(f), Decision::Ordinary); // 24
        assert_eq!(decide(facts()), Decision::Admit);
        let mut f = facts();
        f.record = Document::Missing;
        assert_eq!(decide(f), Decision::Admit);
    }

    #[test]
    fn ordinary_messages_are_preserved() {
        // Compare actual existing path judgement with the pure branch's refusal bytes.
        for path in [
            Path::new("/Volumes/Builds/pool"),
            Path::new("missing/../pool"),
        ] {
            let message = crate::config::refuse_unsafe_pool_path(path).unwrap_err();
            let refusal = crate::config::judge_pool_path(path).unwrap_err();
            let mut f = facts();
            f.ordinary = Err(refusal);
            f.declaration = None;
            let Decision::Refuse(Refusal::Path(result)) = decide(f) else {
                panic!("expected path refusal")
            };
            assert_eq!(result.message(), message);
        }
    }
    #[test]
    fn strict_documents_and_every_identity_field() {
        for (is_binding, document) in [(true, BINDING), (false, RECORD)] {
            for key in ["volume_uuid", "mount_point", "scratch_root", "pool"] {
                let mut value: serde_json::Value = serde_json::from_slice(document).unwrap();
                value[key] = serde_json::Value::String("different".into());
                let bytes = serde_json::to_vec(&value).unwrap();
                let mut f = facts();
                if is_binding {
                    f.binding = Document::Bytes(&bytes);
                } else {
                    f.record = Document::Bytes(&bytes);
                }
                let expected = if is_binding {
                    "scratch-binding-mismatch"
                } else if key == "pool" {
                    "scratch-record-invalid"
                } else {
                    "scratch-record-mismatch"
                };
                assert_eq!(decide(f), class(expected), "{is_binding} {key}");
            }
            for key in ["schema", "unknown"] {
                let mut value: serde_json::Value = serde_json::from_slice(document).unwrap();
                value[key] = serde_json::Value::String("wrong".into());
                let bytes = serde_json::to_vec(&value).unwrap();
                let mut f = facts();
                if is_binding {
                    f.binding = Document::Bytes(&bytes);
                } else {
                    f.record = Document::Bytes(&bytes);
                }
                assert_eq!(
                    decide(f),
                    class(if is_binding {
                        "scratch-binding-invalid"
                    } else {
                        "scratch-record-invalid"
                    })
                );
            }
            let oversized = vec![b' '; if is_binding { 16385 } else { 4097 }];
            let mut f = facts();
            if is_binding {
                f.binding = Document::Bytes(&oversized);
            } else {
                f.record = Document::Bytes(&oversized);
            }
            assert_eq!(
                decide(f),
                class(if is_binding {
                    "scratch-binding-invalid"
                } else {
                    "scratch-record-invalid"
                })
            );
        }
        // created_ms is provenance, so it does not participate in agreement.
        assert_eq!(decide(facts()), Decision::Admit);
    }

    #[test]
    fn internal_success_is_inert_and_status_checks_git() {
        let mut f = facts();
        f.ordinary = Ok(());
        f.source = Source::Default;
        f.root_safe = false;
        assert_eq!(decide(f), Decision::Ordinary);
        let mut f = facts();
        f.status = true;
        f.git_recheck.1 = Err(PathRefusal::GitWorkTree("git".into()));
        assert_eq!(
            decide(f),
            Decision::Unavailable(Refusal::Path(PathRefusal::GitWorkTree("git".into())))
        );
    }
}
