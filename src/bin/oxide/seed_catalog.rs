//! Produce bounded, per-unit catalog evidence from a held producing slot.
//! No source or donor is trusted from a filename alone: every offered output
//! is joined through its own rustc dep-info to a verified source root.

use crate::seed_plan::VerifiedSource;
use oxide_storage::lease::managed::{
    SeedFile, SeedFileKind, SeedJoin, SeedRejectReason, SeedRejection, SeedRole, SeedSourceKind,
    SeedUnit,
};
use oxide_storage::lease::seed::{
    parse_seed_join, seed_output_rule_proven, MAX_JOIN_BYTES, SEED_MAX_FILES,
    SEED_MAX_REJECTED_UNITS, SEED_MAX_REJECTIONS, SEED_MAX_UNITS_PER_PACKAGE,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// This producer's own read budget for one window: catalog file entries and
/// the join file ceiling are the shared catalogue caps; the byte and entry
/// budgets bound work that has no catalog field of its own.
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

pub(crate) struct Producer {
    root: PathBuf,
    root_dir: File,
    started: Instant,
    files: usize,
    bytes: u64,
    entries: usize,
    device: u64,
}

impl Producer {
    pub fn new(root: &Path) -> Option<Self> {
        let metadata = fs::symlink_metadata(root).ok()?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return None;
        }
        let canonical = fs::canonicalize(root).ok()?;
        if canonical != root {
            return None;
        }
        let checked = fs::symlink_metadata(&canonical).ok()?;
        if !checked.is_dir() || checked.dev() != metadata.dev() || checked.ino() != metadata.ino() {
            return None;
        }
        let root_dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&canonical)
            .ok()?;
        let opened = root_dir.metadata().ok()?;
        if opened.dev() != metadata.dev()
            || opened.ino() != metadata.ino()
            || opened.uid() != metadata.uid()
        {
            return None;
        }
        Some(Self {
            root: canonical,
            root_dir,
            started: Instant::now(),
            files: 0,
            bytes: 0,
            entries: 0,
            device: metadata.dev(),
        })
    }

    fn time(&self) -> Option<()> {
        (self.started.elapsed() <= Duration::from_secs(4)).then_some(())
    }

    fn names(&mut self, relative: &Path) -> Option<Vec<String>> {
        self.time()?;
        let directory = self.open_path(relative, true)?;
        // fdopendir consumes its own close-on-exec duplicate.
        let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return None;
        }
        let raw = unsafe { libc::fdopendir(duplicate) };
        if raw.is_null() {
            unsafe { libc::close(duplicate) };
            return None;
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Stream(raw);
        let mut names = Vec::new();
        loop {
            self.time()?;
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            #[cfg(not(target_os = "macos"))]
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                if std::io::Error::last_os_error().raw_os_error() != Some(0) {
                    return None;
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
                .to_str()
                .ok()?;
            if name == "." || name == ".." {
                continue;
            }
            self.entries = self.entries.checked_add(1)?;
            if self.entries > MAX_ENTRIES {
                return None;
            }
            names.push(name.to_owned());
        }
        names.sort();
        Some(names)
    }

    fn open_path(&self, relative: &Path, directory: bool) -> Option<File> {
        let mut current = self.root_dir.try_clone().ok()?;
        let components: Vec<_> = relative.components().collect();
        for (index, component) in components.iter().enumerate() {
            let std::path::Component::Normal(name) = component else {
                return None;
            };
            let name = CString::new(name.as_bytes()).ok()?;
            let is_dir = index + 1 < components.len() || directory;
            let flags = libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | libc::O_CLOEXEC
                | if is_dir { libc::O_DIRECTORY } else { 0 };
            let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return None;
            }
            current = unsafe { File::from_raw_fd(fd) };
            let metadata = current.metadata().ok()?;
            if metadata.uid() != unsafe { libc::geteuid() }
                || metadata.dev() != self.device
                || (is_dir && !metadata.is_dir())
            {
                return None;
            }
        }
        Some(current)
    }

    fn read_file(
        &mut self,
        relative: &str,
        kind: SeedFileKind,
        keep_bytes: bool,
    ) -> Option<(SeedFile, Vec<u8>)> {
        self.time()?;
        let mut file = self.open_path(Path::new(relative), false)?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.dev() != self.device
        {
            return None;
        }
        self.files = self.files.checked_add(1)?;
        self.bytes = self.bytes.checked_add(metadata.len())?;
        if self.files > SEED_MAX_FILES
            || self.bytes > MAX_BYTES
            || (keep_bytes && metadata.len() > MAX_JOIN_BYTES as u64)
        {
            return None;
        }
        let mut hasher = Sha256::new();
        let mut bytes = Vec::new();
        let mut buf = [0u8; 65536];
        let mut count = 0u64;
        loop {
            self.time()?;
            let n = file.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            count = count.checked_add(n as u64)?;
            if count > metadata.len() {
                return None;
            }
            hasher.update(&buf[..n]);
            if keep_bytes {
                bytes.extend_from_slice(&buf[..n]);
            }
        }
        if count != metadata.len() {
            return None;
        }
        let after = file.metadata().ok()?;
        if after.dev() != metadata.dev()
            || after.ino() != metadata.ino()
            || after.len() != metadata.len()
            || after.mtime() != metadata.mtime()
            || after.mtime_nsec() != metadata.mtime_nsec()
        {
            return None;
        }
        let mtime_ms = metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis()
            .try_into()
            .ok()?;
        Some((
            SeedFile {
                path: relative.to_owned(),
                size: count,
                mtime_ms,
                digest: hasher
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                kind,
            },
            bytes,
        ))
    }

    /// Record donor-side candidate unit counts for lock-side rejections.
    /// Ambiguous lock entries sharing one normalized name are grouped so one
    /// fingerprint directory is never counted twice.
    pub fn rejections(
        &mut self,
        profile: &str,
        rejected: &[crate::seed_lock::RejectedPackage],
    ) -> Vec<SeedRejection> {
        let root = format!("{profile}/.fingerprint");
        let names = self.names(Path::new(&root));
        let mut grouped: std::collections::BTreeMap<(String, SeedRejectReason), BTreeSet<String>> =
            std::collections::BTreeMap::new();
        for item in rejected {
            let name = item.package.name.replace('-', "_");
            let key = if name.len() <= 64 {
                name
            } else {
                "unknown".to_owned()
            };
            grouped
                .entry((key, item.reason))
                .or_default()
                .insert(item.package.name.clone());
        }
        let mut result = Vec::new();
        let mut total = 0u64;
        for ((name, reason), literals) in grouped {
            // Keep the whole record inside the catalog cap: the last slot is
            // the bounded "more than we can name" marker.
            if result.len() + 1 >= SEED_MAX_REJECTIONS {
                result.push(SeedRejection {
                    name: "unknown".into(),
                    reason: SeedRejectReason::Cap,
                    units: None,
                });
                break;
            }
            let mut count = Some(0u32);
            if let Some(names) = &names {
                for entry in names {
                    let matches = literals.iter().any(|literal| {
                        entry
                            .strip_prefix(&format!("{literal}-"))
                            .is_some_and(|hash| {
                                hash.len() == 16
                                    && hash.bytes().all(|byte| {
                                        byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                                    })
                            })
                    });
                    if !matches {
                        continue;
                    }
                    let entries = self.names(Path::new(&format!("{root}/{entry}")));
                    let Some(entries) = entries else {
                        count = None;
                        break;
                    };
                    let lib = format!("lib-{name}");
                    let candidates = [lib.as_str(), "build-script-build"];
                    let roles: Vec<&str> = candidates
                        .into_iter()
                        .filter(|stem| entries.iter().any(|entry| entry == stem))
                        .collect();
                    if roles.len() == 1 {
                        let stem = roles[0];
                        let triple = [
                            stem.to_owned(),
                            format!("{stem}.json"),
                            format!("dep-{stem}"),
                        ];
                        let complete = triple.iter().all(|part| entries.contains(part))
                            && entries
                                .iter()
                                .all(|part| triple.contains(part) || part == "invoked.timestamp");
                        if !complete {
                            count = None;
                            break;
                        }
                        count = count.and_then(|value| value.checked_add(1));
                    } else if roles.is_empty()
                        && entries.iter().all(|part| {
                            part == "invoked.timestamp"
                                || part.starts_with("run-build-script-build-script-build")
                                || part == "dep-run-build-script-build-script-build"
                        })
                    {
                        // A classified run-only directory is not a compile unit.
                    } else {
                        count = None;
                        break;
                    }
                }
            } else {
                count = None;
            }
            if let Some(value) = count {
                total = total.saturating_add(u64::from(value));
                if total > SEED_MAX_REJECTED_UNITS {
                    count = None;
                }
            }
            result.push(SeedRejection {
                name,
                reason,
                units: count,
            });
        }
        result
    }

    /// Classify each fingerprint directory independently. Unknown entries make
    /// that directory unusable; a run-only directory is never offered.
    pub fn units(&mut self, profile: &str, source: &VerifiedSource) -> Option<Vec<SeedUnit>> {
        if profile != "debug" && profile != "release" {
            return None;
        }
        // v1 offers only plain registry library compile units. A Git source
        // is already rejected by the lock filter; this guard is defense in
        // depth. Proc-macro and build-script packages are never offered, so
        // a proc macro cannot be reused from a donor and a build-script unit
        // never enters a plan.
        if source.kind != SeedSourceKind::Registry || source.proc_macro {
            return Some(Vec::new());
        }
        let fingerprint_root = format!("{profile}/.fingerprint");
        let names = self.names(Path::new(&fingerprint_root))?;
        let prefix = format!("{}-", source.package.name);
        let mut units = Vec::new();
        for name in names {
            let Some(hash) = name.strip_prefix(&prefix) else {
                continue;
            };
            if hash.len() != 16
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                continue;
            }
            let dir = format!("{fingerprint_root}/{name}");
            let Some(entries) = self.names(Path::new(&dir)) else {
                continue;
            };
            let target = source.package.name.replace('-', "_");
            let lib = format!("lib-{target}");
            // A build-script compile directory has no `lib-<target>` entry;
            // v1 never classifies it. Only a library unit is considered.
            if !entries.iter().any(|name| name == &lib) {
                continue;
            }
            let stem = lib.as_str();
            let expected: BTreeSet<String> = [
                stem.to_owned(),
                format!("{stem}.json"),
                format!("dep-{stem}"),
            ]
            .into_iter()
            .collect();
            if !expected.iter().all(|name| entries.contains(name)) {
                continue;
            }
            if entries
                .iter()
                .any(|name| !expected.contains(name) && name != "invoked.timestamp")
            {
                continue;
            }
            let role = SeedRole::Lib;
            let output_dir = format!("{profile}/deps");
            let dep = format!("{output_dir}/{target}-{hash}.d");
            // A library unit's outputs are the `.rmeta` and, when the command
            // emitted one, the `.rlib`: `cargo check` produces the metadata
            // only, and a check-built donor is a supported donor (§3, §5.1).
            // Every artifact that does exist is joined through its own rule.
            let mut artifacts = Vec::new();
            for extension in ["rmeta", "rlib"] {
                let path = format!("{output_dir}/lib{target}-{hash}.{extension}");
                if self.open_path(Path::new(&path), false).is_some() {
                    artifacts.push(path);
                }
            }
            if artifacts.is_empty() {
                continue;
            }
            let Some((dep_file, dep_bytes)) = self.read_file(&dep, SeedFileKind::DepInfo, true)
            else {
                continue;
            };
            let Ok(parsed) = parse_seed_join(&dep_bytes) else {
                continue;
            };
            if parsed.inputs.is_empty()
                || parsed.inputs.iter().any(|input| {
                    let path = Path::new(input);
                    !path.is_absolute()
                        || !path.starts_with(&source.root)
                        || path
                            .components()
                            .any(|part| matches!(part, std::path::Component::ParentDir))
                })
            {
                continue;
            }
            let inputs: BTreeSet<&str> = parsed.inputs.iter().map(String::as_str).collect();
            let outputs: BTreeSet<String> = std::iter::once(dep.as_str())
                .chain(artifacts.iter().map(String::as_str))
                .flat_map(|path| {
                    [
                        path.to_owned(),
                        self.root.join(path).to_string_lossy().into_owned(),
                    ]
                })
                .collect();
            if parsed.rules.iter().any(|rule| {
                rule.targets.len() != 1
                    || if outputs.contains(&rule.targets[0]) {
                        rule.inputs.is_empty()
                            || rule
                                .inputs
                                .iter()
                                .any(|input| !inputs.contains(input.as_str()))
                    } else {
                        !rule.inputs.is_empty() || !inputs.contains(rule.targets[0].as_str())
                    }
            }) || !seed_output_rule_proven(&parsed, &self.root, &dep, &parsed.inputs)
                || artifacts.iter().any(|artifact| {
                    !seed_output_rule_proven(&parsed, &self.root, artifact, &parsed.inputs)
                })
            {
                continue;
            }
            let mut files = Vec::new();
            let mut incomplete = false;
            for entry in &expected {
                match self.read_file(&format!("{dir}/{entry}"), SeedFileKind::Fingerprint, false) {
                    Some((file, _)) => files.push(file),
                    None => {
                        incomplete = true;
                        break;
                    }
                }
            }
            if incomplete {
                continue;
            }
            for artifact in &artifacts {
                match self.read_file(artifact, SeedFileKind::Artifact, false) {
                    Some((file, _)) => files.push(file),
                    None => {
                        incomplete = true;
                        break;
                    }
                }
            }
            if incomplete {
                continue;
            }
            let digest = dep_file.digest.clone();
            files.push(dep_file);
            units.push(SeedUnit {
                fingerprint_dir: dir,
                role,
                target,
                join: SeedJoin {
                    path: dep,
                    targets: parsed.targets,
                    inputs: parsed.inputs,
                    digest,
                },
                files,
            });
            if units.len() > SEED_MAX_UNITS_PER_PACKAGE {
                return None;
            }
        }
        self.time()?;
        Some(units)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_storage::lease::managed::SeedSourceKind;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let template = std::env::temp_dir().join("oxide-seed-catalog.XXXXXX");
            let mut bytes = CString::new(template.as_os_str().as_bytes())
                .unwrap()
                .into_bytes_with_nul();
            // SAFETY: the buffer is writable, NUL terminated and ends in XXXXXX.
            assert!(!unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) }.is_null());
            let path = std::ffi::CStr::from_bytes_until_nul(&bytes).unwrap();
            Self(
                fs::canonicalize(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
                    .unwrap(),
            )
        }
        fn write(&self, name: &str, content: &str) {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn descriptor_walk_refuses_symlinked_parents_and_wrong_device() {
        let fixture = Fixture::new();
        fixture.write("good/file", "data");
        std::os::unix::fs::symlink(fixture.0.join("good"), fixture.0.join("link")).unwrap();
        let mut producer = Producer::new(&fixture.0).unwrap();
        assert!(producer
            .read_file("link/file", SeedFileKind::Artifact, false)
            .is_none());
        assert!(producer.names(Path::new("link")).is_none());
        assert!(producer
            .read_file("good/file", SeedFileKind::Artifact, false)
            .is_some());
        producer.device = producer.device.wrapping_add(1);
        assert!(producer
            .read_file("good/file", SeedFileKind::Artifact, false)
            .is_none());
    }

    #[test]
    fn joins_complete_unit_with_real_absolute_dep_info_and_rejects_foreign_source() {
        let fixture = Fixture::new();
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source_root = fixture.0.join("source");
        fixture.write("source/src/lib.rs", "source");
        let source = VerifiedSource {
            package,
            tree_digest: "b".repeat(64),
            kind: SeedSourceKind::Registry,
            root: source_root.clone(),
            verification_root: source_root.clone(),
            proc_macro: false,
        };
        let dir = "build/debug/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa";
        for name in ["lib-cfg_if", "lib-cfg_if.json", "dep-lib-cfg_if"] {
            fixture.write(&format!("{dir}/{name}"), "fingerprint");
        }
        let dep = "debug/deps/cfg_if-aaaaaaaaaaaaaaaa.d";
        let artifact = "debug/deps/libcfg_if-aaaaaaaaaaaaaaaa.rmeta";
        fixture.write(&format!("build/{artifact}"), "artifact");
        let build = fixture.0.join("build");
        let input = source_root.join("src/lib.rs");
        let contents = format!(
            "{}: {}\n{}: {}\n{}:\n",
            build.join(dep).display(),
            input.display(),
            build.join(artifact).display(),
            input.display(),
            input.display()
        );
        fixture.write(&format!("build/{dep}"), &contents);
        let units = Producer::new(&build)
            .unwrap()
            .units("debug", &source)
            .unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].files.len(), 5);
        fixture.write(
            &format!("build/{dep}"),
            &contents.replace(&input.to_string_lossy().to_string(), "/foreign/lib.rs"),
        );
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn build_script_and_proc_macro_units_are_never_offered() {
        let fixture = Fixture::new();
        let lock = format!("version = 4\n[[package]]\nname = 'seedctl'\nversion = '0.1.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source_root = fixture.0.join("source");
        fixture.write("source/src/lib.rs", "source");
        let hash = "aaaaaaaaaaaaaaaa";
        let dir = format!("build/debug/.fingerprint/seedctl-{hash}");
        for name in [
            "build-script-build",
            "build-script-build.json",
            "dep-build-script-build",
        ] {
            fixture.write(&format!("{dir}/{name}"), "fingerprint");
        }
        let build = fixture.0.join("build");
        let build_source = VerifiedSource {
            package: package.clone(),
            tree_digest: "b".repeat(64),
            kind: SeedSourceKind::Registry,
            root: source_root.clone(),
            verification_root: source_root.clone(),
            proc_macro: false,
        };
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &build_source)
            .unwrap()
            .is_empty());
        // A complete proc-macro library fingerprint is still never offered.
        let pm_dir = format!("build/debug/.fingerprint/seedpm-{hash}");
        for name in ["lib-seedpm", "lib-seedpm.json", "dep-lib-seedpm"] {
            fixture.write(&format!("{pm_dir}/{name}"), "fingerprint");
        }
        fixture.write(&format!("build/debug/deps/libseedpm-{hash}.dylib"), "dylib");
        fixture.write(
            &format!("build/debug/deps/seedpm-{hash}.d"),
            &format!(
                "{}: {}\n{}: {}\n{}:\n",
                build.join(format!("debug/deps/seedpm-{hash}.d")).display(),
                source_root.join("src/lib.rs").display(),
                build
                    .join(format!("debug/deps/libseedpm-{hash}.dylib"))
                    .display(),
                source_root.join("src/lib.rs").display(),
                source_root.join("src/lib.rs").display()
            ),
        );
        let pm_source = VerifiedSource {
            package: crate::seed_lock::eligible(
                b"version = 4\n[[package]]\nname = 'seedpm'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\n",
            )
            .unwrap()
            .remove(0),
            tree_digest: "b".repeat(64),
            kind: SeedSourceKind::Registry,
            root: source_root.clone(),
            verification_root: source_root,
            proc_macro: true,
        };
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &pm_source)
            .unwrap()
            .is_empty());
    }

    /// One complete library unit: the fingerprint triple, an rmeta artifact
    /// and its joined `.d`, all naming one hash and the verified source.
    fn write_complete_unit(fixture: &Fixture, hash: &str, input: &Path) {
        let dir = format!("build/debug/.fingerprint/cfg-if-{hash}");
        for name in ["lib-cfg_if", "lib-cfg_if.json", "dep-lib-cfg_if"] {
            fixture.write(&format!("{dir}/{name}"), "fingerprint");
        }
        let build = fixture.0.join("build");
        let artifact = format!("build/debug/deps/libcfg_if-{hash}.rmeta");
        fixture.write(&artifact, "artifact");
        fixture.write(
            &format!("build/debug/deps/cfg_if-{hash}.d"),
            &format!(
                "{}: {}\n{}: {}\n{}:\n",
                build.join(format!("debug/deps/cfg_if-{hash}.d")).display(),
                input.display(),
                build
                    .join(format!("debug/deps/libcfg_if-{hash}.rmeta"))
                    .display(),
                input.display(),
                input.display()
            ),
        );
    }

    #[test]
    fn unknown_fingerprint_entries_and_a_fourth_unit_offer_nothing() {
        let fixture = Fixture::new();
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source_root = fixture.0.join("source");
        fixture.write("source/src/lib.rs", "source");
        let source = VerifiedSource {
            package,
            tree_digest: "b".repeat(64),
            kind: SeedSourceKind::Registry,
            root: source_root.clone(),
            verification_root: source_root.clone(),
            proc_macro: false,
        };
        let input = source_root.join("src/lib.rs");
        let build = fixture.0.join("build");
        write_complete_unit(&fixture, "aaaaaaaaaaaaaaaa", &input);
        assert_eq!(
            Producer::new(&build)
                .unwrap()
                .units("debug", &source)
                .unwrap()
                .len(),
            1
        );
        // An entry this layer cannot classify makes that directory unusable.
        fixture.write(
            "build/debug/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa/unclassified",
            "x",
        );
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source)
            .unwrap()
            .is_empty());
        fs::remove_file(
            fixture
                .0
                .join("build/debug/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa/unclassified"),
        )
        .unwrap();
        // Three units are the per-package ceiling; a fourth refuses the whole
        // package rather than truncating it.
        for hash in ["bbbbbbbbbbbbbbbb", "cccccccccccccccc"] {
            write_complete_unit(&fixture, hash, &input);
        }
        assert_eq!(
            Producer::new(&build)
                .unwrap()
                .units("debug", &source)
                .unwrap()
                .len(),
            3
        );
        write_complete_unit(&fixture, "dddddddddddddddd", &input);
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source)
            .is_none());
    }

    #[test]
    fn producer_refuses_a_symlinked_build_root() {
        let fixture = Fixture::new();
        let build = fixture.0.join("build");
        fs::create_dir(&build).unwrap();
        let link = fixture.0.join("link");
        std::os::unix::fs::symlink(&build, &link).unwrap();
        assert!(Producer::new(&build).is_some());
        assert!(Producer::new(&link).is_none());
        fs::create_dir(fixture.0.join("container")).unwrap();
        std::os::unix::fs::symlink(&fixture.0, fixture.0.join("container/alias")).unwrap();
        assert!(Producer::new(&fixture.0.join("container/alias/build")).is_none());
    }
    #[test]
    fn ambiguous_name_counts_classifiable_units_and_marks_unknown() {
        let fixture = Fixture::new();
        let lock = "version = 4\n[[package]]\nname = 'foo-bar'\nversion = '1'\n[[package]]\nname = 'foo_bar'\nversion = '1'\n";
        let analysis = crate::seed_lock::analyze(lock.as_bytes()).unwrap();
        let first = "build/debug/.fingerprint/foo-bar-aaaaaaaaaaaaaaaa";
        for name in ["lib-foo_bar", "lib-foo_bar.json", "dep-lib-foo_bar"] {
            fixture.write(&format!("{first}/{name}"), "x");
        }
        let second = "build/debug/.fingerprint/foo_bar-bbbbbbbbbbbbbbbb";
        for name in ["lib-foo_bar", "lib-foo_bar.json", "dep-lib-foo_bar"] {
            fixture.write(&format!("{second}/{name}"), "x");
        }
        let root = fixture.0.join("build");
        let records = Producer::new(&root)
            .unwrap()
            .rejections("debug", &analysis.rejected);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].reason, SeedRejectReason::AmbiguousName);
        assert_eq!(records[0].units, Some(2));
        fixture.write(&format!("{second}/unclassified"), "x");
        let records = Producer::new(&root)
            .unwrap()
            .rejections("debug", &analysis.rejected);
        assert_eq!(records[0].units, None);
    }
}
