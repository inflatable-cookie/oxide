//! Produce bounded, per-unit catalog evidence from a held producing slot.
//! Each library artifact is joined through rustc dep-info to a verified source
//! root. The layout v2 adapter also records the unit's split-debuginfo objects.

use crate::seed_plan::VerifiedSource;
use oxide_storage::lease::managed::{
    SeedFile, SeedFileKind, SeedJoin, SeedRejectReason, SeedRejection, SeedRole, SeedSourceKind,
    SeedUnit,
};
use oxide_storage::lease::seed::catalog::is_v2_debug_object;
use oxide_storage::lease::seed::{
    parse_seed_join, seed_output_rule_proven, SeedLayout, MAX_JOIN_BYTES, SEED_MAX_FILES,
    SEED_MAX_REJECTED_UNITS, SEED_MAX_REJECTIONS, SEED_MAX_UNITS_PER_PACKAGE, SEED_MAX_UNIT_FILES,
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
    /// unit directory is never counted twice.
    pub fn rejections(
        &mut self,
        profile: &str,
        rejected: &[crate::seed_lock::RejectedPackage],
        layout: SeedLayout,
    ) -> Vec<SeedRejection> {
        // Layout v1 lists the profile's `.fingerprint` once for the whole
        // record. Layout v2 counts beneath each package's own directory.
        let v1_names = match layout {
            SeedLayout::V1 => self.names(Path::new(&format!("{profile}/.fingerprint"))),
            SeedLayout::V2 => None,
        };
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
            let mut count = match layout {
                SeedLayout::V1 => self.count_v1(profile, v1_names.as_deref(), &name, &literals),
                SeedLayout::V2 => self.count_v2(profile, &name, &literals),
            };
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

    /// Layout v1 candidate units under the profile's `.fingerprint` listing.
    /// `None` means the candidates could not all be classified.
    fn count_v1(
        &mut self,
        profile: &str,
        names: Option<&[String]>,
        name: &str,
        literals: &BTreeSet<String>,
    ) -> Option<u32> {
        let names = names?;
        let root = format!("{profile}/.fingerprint");
        let mut count = Some(0u32);
        for entry in names {
            let matches = literals.iter().any(|literal| {
                entry
                    .strip_prefix(&format!("{literal}-"))
                    .is_some_and(is_unit_hash)
            });
            if !matches {
                continue;
            }
            let entries = self.names(Path::new(&format!("{root}/{entry}")))?;
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
                    return None;
                }
                count = count.and_then(|value| value.checked_add(1));
            } else if roles.is_empty() && entries.iter().all(|part| is_run_only_entry(part)) {
                // A classified run-only directory is not a compile unit.
            } else {
                return None;
            }
        }
        count
    }

    /// Layout v2 candidate units beneath each literal package directory. A
    /// package with no directory in this profile holds no units. Anything the
    /// producer cannot classify is unknown, never zero.
    fn count_v2(&mut self, profile: &str, name: &str, literals: &BTreeSet<String>) -> Option<u32> {
        let lib = format!("lib-{name}");
        let stems = [lib.clone(), format!("{lib}.json"), format!("dep-{lib}")];
        let mut count = 0u32;
        for literal in literals {
            let package = format!("{profile}/build/{literal}");
            if self.open_path(Path::new(&package), true).is_none() {
                continue;
            }
            for hash in self.names(Path::new(&package))? {
                if !is_unit_hash(&hash) {
                    return None;
                }
                let unit = format!("{package}/{hash}");
                let entries = self.names(Path::new(&unit))?;
                let fingerprint = self.names(Path::new(&format!("{unit}/fingerprint")))?;
                if fingerprint.contains(&lib) {
                    let complete = stems.iter().all(|stem| fingerprint.contains(stem));
                    let known = fingerprint.iter().all(|entry| {
                        stems.contains(entry)
                            || entry == "invoked.timestamp"
                            || *entry == format!("output-{lib}")
                    });
                    let bounded = entries
                        .iter()
                        .all(|entry| entry == "fingerprint" || entry == "out");
                    if !complete || !known || !bounded {
                        return None;
                    }
                    count = count.checked_add(1)?;
                } else if !fingerprint.iter().all(|entry| is_run_only_entry(entry)) {
                    return None;
                }
            }
        }
        Some(count)
    }

    /// Classify each library unit of one package independently. Unknown
    /// entries make that unit unusable, and it is not offered. Layout v1 and
    /// layout v2 use their own unit shapes and share one join check.
    pub fn units(
        &mut self,
        profile: &str,
        source: &VerifiedSource,
        layout: SeedLayout,
    ) -> Option<Vec<SeedUnit>> {
        if profile != "debug" && profile != "release" {
            return None;
        }
        // Only plain registry library compile units are offered. A Git source
        // is already rejected by the lock filter; this guard is defense in
        // depth. Proc-macro and build-script packages are never offered, so a
        // proc macro cannot be reused from a donor and a build-script unit never
        // enters a plan.
        if source.kind != SeedSourceKind::Registry || source.proc_macro {
            return Some(Vec::new());
        }
        let units = match layout {
            SeedLayout::V1 => self.units_v1(profile, source)?,
            SeedLayout::V2 => self.units_v2(profile, source)?,
        };
        self.time()?;
        Some(units)
    }

    fn units_v1(&mut self, profile: &str, source: &VerifiedSource) -> Option<Vec<SeedUnit>> {
        let fingerprint_root = format!("{profile}/.fingerprint");
        let names = self.names(Path::new(&fingerprint_root))?;
        let prefix = format!("{}-", source.package.name);
        let target = source.package.name.replace('-', "_");
        let lib = format!("lib-{target}");
        let mut units = Vec::new();
        for name in names {
            let Some(hash) = name.strip_prefix(&prefix) else {
                continue;
            };
            if !is_unit_hash(hash) {
                continue;
            }
            let dir = format!("{fingerprint_root}/{name}");
            let Some(entries) = self.names(Path::new(&dir)) else {
                continue;
            };
            // A build-script compile directory has no `lib-<target>` entry;
            // v1 never classifies it. Only a library unit is considered.
            if !entries.iter().any(|entry| entry == &lib) {
                continue;
            }
            let expected: BTreeSet<String> =
                [lib.clone(), format!("{lib}.json"), format!("dep-{lib}")]
                    .into_iter()
                    .collect();
            if !expected.iter().all(|entry| entries.contains(entry)) {
                continue;
            }
            if entries
                .iter()
                .any(|entry| !expected.contains(entry) && entry != "invoked.timestamp")
            {
                continue;
            }
            // v1 keeps a unit's fingerprint triple in its directory and its
            // outputs in `<profile>/deps`.
            let shape = UnitShape {
                dir: dir.clone(),
                fingerprint: dir,
                outputs: format!("{profile}/deps"),
                hash: hash.to_owned(),
            };
            let fingerprint: Vec<String> = expected.into_iter().collect();
            if let Some(unit) = self.offer_lib_unit(source, &shape, &fingerprint, None) {
                units.push(unit);
            }
            if units.len() > SEED_MAX_UNITS_PER_PACKAGE {
                return None;
            }
        }
        Some(units)
    }

    /// Layout v2 library units, one per `<profile>/build/<package>/<hex>`
    /// directory. The unit's fingerprint files live in `fingerprint/` and its
    /// outputs in `out/`, including any split-debuginfo objects.
    fn units_v2(&mut self, profile: &str, source: &VerifiedSource) -> Option<Vec<SeedUnit>> {
        // A profile that holds layout v1 output is not a v2 producer's to
        // classify: the Cargo that wrote it is not the one this adapter names.
        for v1 in [format!("{profile}/deps"), format!("{profile}/.fingerprint")] {
            if self.open_path(Path::new(&v1), true).is_some() {
                return None;
            }
        }
        let package = format!("{profile}/build/{}", source.package.name);
        // No directory for this package in the profile holds no units.
        if self.open_path(Path::new(&package), true).is_none() {
            return Some(Vec::new());
        }
        let target = source.package.name.replace('-', "_");
        let lib = format!("lib-{target}");
        let required = [lib.clone(), format!("{lib}.json"), format!("dep-{lib}")];
        let optional = [format!("output-{lib}"), "invoked.timestamp".to_owned()];
        let mut units = Vec::new();
        for hash in self.names(Path::new(&package))? {
            if !is_unit_hash(&hash) {
                continue;
            }
            let dir = format!("{package}/{hash}");
            // A library unit has exactly its `fingerprint/` and `out/`.
            let Some(entries) = self.names(Path::new(&dir)) else {
                continue;
            };
            if entries
                .iter()
                .any(|entry| entry != "fingerprint" && entry != "out")
            {
                continue;
            }
            let fingerprint_dir = format!("{dir}/fingerprint");
            let Some(fingerprint) = self.names(Path::new(&fingerprint_dir)) else {
                continue;
            };
            // A build-script or run-only directory has no library triple.
            if !fingerprint.contains(&lib) {
                continue;
            }
            if !required.iter().all(|entry| fingerprint.contains(entry))
                || fingerprint
                    .iter()
                    .any(|entry| !required.contains(entry) && !optional.contains(entry))
            {
                continue;
            }
            let outputs_dir = format!("{dir}/out");
            let Some(outputs) = self.names(Path::new(&outputs_dir)) else {
                continue;
            };
            if fingerprint.len() + outputs.len() > SEED_MAX_UNIT_FILES {
                continue;
            }
            let shape = UnitShape {
                dir,
                fingerprint: fingerprint_dir,
                outputs: outputs_dir,
                hash: hash.clone(),
            };
            if let Some(unit) = self.offer_lib_unit(source, &shape, &fingerprint, Some(&outputs)) {
                units.push(unit);
            }
            if units.len() > SEED_MAX_UNITS_PER_PACKAGE {
                return None;
            }
        }
        Some(units)
    }

    /// Join one library unit's `.d` to its verified source root and read its
    /// files. `None` means the unit is not offered: its join is not proven, a
    /// file cannot be read, or it exceeds a catalog cap.
    fn offer_lib_unit(
        &mut self,
        source: &VerifiedSource,
        shape: &UnitShape,
        fingerprint: &[String],
        listed_outputs: Option<&[String]>,
    ) -> Option<SeedUnit> {
        let target = source.package.name.replace('-', "_");
        let stem = format!("{target}-{}", shape.hash);
        let dep_name = format!("{stem}.d");
        let dep = format!("{}/{}", shape.outputs, dep_name);
        // A library unit's outputs are the `.rmeta` and, when the command
        // emitted one, the `.rlib`: `cargo check` produces the metadata only,
        // and a check-built donor is a supported donor (§3, §5.1). Every
        // artifact that does exist is joined through its own rule.
        let mut artifacts = Vec::new();
        for extension in ["rmeta", "rlib"] {
            let name = format!("lib{stem}.{extension}");
            let path = format!("{}/{}", shape.outputs, name);
            let present = match listed_outputs {
                Some(names) => names.contains(&name),
                None => self.open_path(Path::new(&path), false).is_some(),
            };
            if present {
                artifacts.push(path);
            }
        }
        if artifacts.is_empty() {
            return None;
        }
        let mut objects = Vec::new();
        if let Some(names) = listed_outputs {
            let prefix = format!("{stem}.");
            for name in names {
                let path = format!("{}/{}", shape.outputs, name);
                if path == dep || artifacts.contains(&path) {
                    continue;
                }
                if !is_v2_debug_object(name, &prefix) {
                    return None;
                }
                objects.push(path);
            }
        }
        let (dep_file, dep_bytes) = self.read_file(&dep, SeedFileKind::DepInfo, true)?;
        let parsed = parse_seed_join(&dep_bytes).ok()?;
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
            return None;
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
            return None;
        }
        let mut files = Vec::new();
        for entry in fingerprint {
            let (file, _) = self.read_file(
                &format!("{}/{entry}", shape.fingerprint),
                SeedFileKind::Fingerprint,
                false,
            )?;
            files.push(file);
        }
        for artifact in &artifacts {
            let (file, _) = self.read_file(artifact, SeedFileKind::Artifact, false)?;
            files.push(file);
        }
        for object in &objects {
            let (file, _) = self.read_file(object, SeedFileKind::DebugObject, false)?;
            files.push(file);
        }
        let digest = dep_file.digest.clone();
        files.push(dep_file);
        if files.len() > SEED_MAX_UNIT_FILES {
            return None;
        }
        Some(SeedUnit {
            fingerprint_dir: shape.dir.clone(),
            role: SeedRole::Lib,
            target,
            join: SeedJoin {
                path: dep,
                targets: parsed.targets,
                inputs: parsed.inputs,
                digest,
            },
            files,
        })
    }
}

/// One library unit's paths under a layout.
struct UnitShape {
    /// The unit directory, relative to the slot's `build/`: the catalog's
    /// `fingerprint_dir`.
    dir: String,
    /// The directory holding the fingerprint triple.
    fingerprint: String,
    /// The directory holding the `.d`, the artifacts and debuginfo objects.
    outputs: String,
    /// The 16-hex unit identifier.
    hash: String,
}

/// Sixteen lowercase hexadecimal digits: Cargo's unit identifier.
fn is_unit_hash(text: &str) -> bool {
    text.len() == 16
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// An entry of a run-only or build-script directory, which is not a library
/// compile unit.
fn is_run_only_entry(entry: &str) -> bool {
    entry == "invoked.timestamp"
        || entry.starts_with("run-build-script-build-script-build")
        || entry.starts_with("dep-run-build-script-build-script-build")
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
            .units("debug", &source, SeedLayout::V1)
            .unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].files.len(), 5);
        fixture.write(
            &format!("build/{dep}"),
            &contents.replace(&input.to_string_lossy().to_string(), "/foreign/lib.rs"),
        );
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V1)
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
            .units("debug", &build_source, SeedLayout::V1)
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
            .units("debug", &pm_source, SeedLayout::V1)
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
                .units("debug", &source, SeedLayout::V1)
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
            .units("debug", &source, SeedLayout::V1)
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
                .units("debug", &source, SeedLayout::V1)
                .unwrap()
                .len(),
            3
        );
        write_complete_unit(&fixture, "dddddddddddddddd", &input);
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V1)
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
        let records =
            Producer::new(&root)
                .unwrap()
                .rejections("debug", &analysis.rejected, SeedLayout::V1);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].reason, SeedRejectReason::AmbiguousName);
        assert_eq!(records[0].units, Some(2));
        fixture.write(&format!("{second}/unclassified"), "x");
        let records =
            Producer::new(&root)
                .unwrap()
                .rejections("debug", &analysis.rejected, SeedLayout::V1);
        assert_eq!(records[0].units, None);
    }

    /// The v2 library source of a synthetic `cfg-if` tree: a verified source
    /// root, and a build directory the producer walks.
    fn v2_source(fixture: &Fixture) -> (VerifiedSource, PathBuf) {
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let root = fixture.0.join("source");
        fixture.write("source/src/lib.rs", "source");
        let source = VerifiedSource {
            package,
            tree_digest: "b".repeat(64),
            kind: SeedSourceKind::Registry,
            verification_root: root.clone(),
            root,
            proc_macro: false,
        };
        (source, fixture.0.join("build"))
    }

    /// One complete layout v2 `cfg-if` unit: `fingerprint/` with the triple,
    /// `output-lib` and `invoked.timestamp`, and `out/` with the artifact, the
    /// `.d` and a rustc debuginfo object.
    fn write_v2_unit(fixture: &Fixture, build: &Path, hash: &str, input: &Path) {
        let dir = format!("debug/build/cfg-if/{hash}");
        for name in [
            "lib-cfg_if",
            "lib-cfg_if.json",
            "dep-lib-cfg_if",
            "output-lib-cfg_if",
            "invoked.timestamp",
        ] {
            fixture.write(&format!("build/{dir}/fingerprint/{name}"), "fingerprint");
        }
        fixture.write(
            &format!("build/{dir}/out/libcfg_if-{hash}.rmeta"),
            "artifact",
        );
        fixture.write(
            &format!("build/{dir}/out/cfg_if-{hash}.cfg_if.aaaa-cgu.0.rcgu.o"),
            "object",
        );
        let dep = build.join(format!("{dir}/out/cfg_if-{hash}.d"));
        let artifact = build.join(format!("{dir}/out/libcfg_if-{hash}.rmeta"));
        fixture.write(
            &format!("build/{dir}/out/cfg_if-{hash}.d"),
            &format!(
                "{}: {}\n{}: {}\n{}:\n",
                dep.display(),
                input.display(),
                artifact.display(),
                input.display(),
                input.display()
            ),
        );
    }

    #[test]
    fn layout_v2_offers_the_whole_unit_including_its_debug_object() {
        let fixture = Fixture::new();
        let (source, build) = v2_source(&fixture);
        let hash = "0123456789abcdef";
        write_v2_unit(&fixture, &build, hash, &source.root.join("src/lib.rs"));
        let units = Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V2)
            .expect("layout v2 units");
        assert_eq!(units.len(), 1, "{units:?}");
        assert_eq!(
            units[0].fingerprint_dir,
            format!("debug/build/cfg-if/{hash}")
        );
        let files = &units[0].files;
        assert_eq!(files.len(), 8, "{files:?}");
        assert_eq!(
            files
                .iter()
                .filter(|file| file.kind == SeedFileKind::Fingerprint)
                .count(),
            5
        );
        let object =
            format!("debug/build/cfg-if/{hash}/out/cfg_if-{hash}.cfg_if.aaaa-cgu.0.rcgu.o");
        assert!(files
            .iter()
            .any(|file| { file.kind == SeedFileKind::DebugObject && file.path == object }));
        assert!(!units[0]
            .join
            .targets
            .iter()
            .any(|path| path.ends_with(&object)));
    }

    #[test]
    fn layout_v2_refuses_unknown_or_unreadable_outputs() {
        let fixture = Fixture::new();
        let (source, build) = v2_source(&fixture);
        let hash = "0123456789abcdef";
        write_v2_unit(&fixture, &build, hash, &source.root.join("src/lib.rs"));
        let out = format!("build/debug/build/cfg-if/{hash}/out");
        let unknown = format!("{out}/cfg_if-{hash}.stray.o");
        fixture.write(&unknown, "unknown object name");
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V2)
            .unwrap()
            .is_empty());
        fs::remove_file(fixture.0.join(unknown)).unwrap();

        // A known name must be readable as an owned regular file.
        std::os::unix::fs::symlink(
            format!("libcfg_if-{hash}.rmeta"),
            fixture.0.join(format!("{out}/libcfg_if-{hash}.rlib")),
        )
        .unwrap();
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V2)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn layout_v2_refuses_unknown_entries_and_v1_profiles() {
        let fixture = Fixture::new();
        let (source, build) = v2_source(&fixture);
        let hash = "0123456789abcdef";
        write_v2_unit(&fixture, &build, hash, &source.root.join("src/lib.rs"));
        // An entry the layout does not write makes the unit unusable, never a copy.
        fixture.write(&format!("build/debug/build/cfg-if/{hash}/run/output"), "x");
        let units = Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V2)
            .expect("layout v2 units");
        assert!(units.is_empty(), "{units:?}");
        fs::remove_dir_all(build.join(format!("debug/build/cfg-if/{hash}/run"))).unwrap();
        // A profile that holds layout v1 output is not a v2 producer's to read.
        fixture.write("build/debug/deps/keep", "x");
        assert!(Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V2)
            .is_none());
    }

    #[test]
    fn layout_v2_counts_candidates_and_never_reports_unknown_as_zero() {
        let fixture = Fixture::new();
        let (source, build) = v2_source(&fixture);
        let hash = "0123456789abcdef";
        write_v2_unit(&fixture, &build, hash, &source.root.join("src/lib.rs"));
        let rejected = vec![crate::seed_lock::RejectedPackage {
            package: source.package.clone(),
            reason: SeedRejectReason::NonLeaf,
        }];
        let records = Producer::new(&build)
            .unwrap()
            .rejections("debug", &rejected, SeedLayout::V2);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].units, Some(1));
        // An entry the producer cannot classify makes the count unknown.
        fixture.write(&format!("build/debug/build/cfg-if/{hash}/mystery/x"), "x");
        let records = Producer::new(&build)
            .unwrap()
            .rejections("debug", &rejected, SeedLayout::V2);
        assert_eq!(records[0].units, None);
    }

    /// Real Cargo 1.100.0 output for a registry leaf: the unit is offered whole,
    /// including any split-debuginfo objects on this platform.
    #[test]
    fn layout_v2_offers_the_unit_real_cargo_1_100_writes() {
        use crate::cargo_layout::support::{cargo_1_100, toolchain_installed};
        if !toolchain_installed() {
            eprintln!("skipped: Cargo 1.100.0 is not installed");
            return;
        }
        let fixture = Fixture::new();
        fixture.write(
            "probe/Cargo.toml",
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nitoa = \"=1.0.18\"\n",
        );
        fixture.write(
            "probe/src/lib.rs",
            "pub fn f() -> usize { itoa::Buffer::new().format(7u32).len() }\n",
        );
        let build = fixture.0.join("build");
        let built = cargo_1_100(&fixture.0.join("probe"))
            .args(["build", "--offline"])
            .env("CARGO_BUILD_BUILD_DIR", &build)
            .env("CARGO_TARGET_DIR", fixture.0.join("target"))
            .output()
            .expect("cargo 1.100.0");
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let unit = fs::read_dir(build.join("debug/build/itoa"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .next()
            .expect("the itoa unit");
        let hash = unit.file_name().unwrap().to_string_lossy().into_owned();
        // The registry root is the verified source the `.d` names.
        let dep = fs::read_to_string(unit.join(format!("out/itoa-{hash}.d"))).unwrap();
        let root = dep
            .split_whitespace()
            .map(Path::new)
            .find(|path| path.is_absolute() && path.extension().is_some_and(|ext| ext == "rs"))
            .and_then(|path| path.ancestors().find(|a| a.ends_with("itoa-1.0.18")))
            .expect("the registry root")
            .to_path_buf();
        let lock = "version = 4\n[[package]]\nname = 'itoa'\nversion = '1.0.18'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682'\n";
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source = VerifiedSource {
            package,
            tree_digest: "b".repeat(64),
            kind: SeedSourceKind::Registry,
            verification_root: root.clone(),
            root,
            proc_macro: false,
        };
        let units = Producer::new(&build)
            .unwrap()
            .units("debug", &source, SeedLayout::V2)
            .expect("layout v2 units");
        assert_eq!(units.len(), 1, "{units:?}");
        assert_eq!(units[0].fingerprint_dir, format!("debug/build/itoa/{hash}"));
        let on_disk: BTreeSet<_> = ["fingerprint", "out"]
            .into_iter()
            .flat_map(|directory| fs::read_dir(unit.join(directory)).unwrap())
            .map(|entry| {
                entry
                    .unwrap()
                    .path()
                    .strip_prefix(&build)
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let catalogued = units[0]
            .files
            .iter()
            .map(|file| PathBuf::from(&file.path))
            .collect();
        assert_eq!(on_disk, catalogued);
        assert!(units[0]
            .files
            .iter()
            .any(|file| file.path.ends_with(".rmeta")));
    }
}
