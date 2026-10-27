//! Produce bounded, per-unit catalog evidence from a held producing slot.
//! No source or donor is trusted from a filename alone: every offered output
//! is joined through its own rustc dep-info to a verified source root.

use crate::seed_plan::VerifiedSource;
use oxide_storage::lease::managed::{SeedFile, SeedFileKind, SeedJoin, SeedRole, SeedUnit};
use oxide_storage::lease::seed::{parse_seed_join, seed_output_rule_proven};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

const MAX_FILES: usize = 8192;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DEP: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

pub(crate) struct Producer {
    root: PathBuf,
    started: Instant,
    files: usize,
    bytes: u64,
    entries: usize,
    device: u64,
}

impl Producer {
    pub fn new(root: &Path) -> Option<Self> {
        let root = fs::canonicalize(root).ok()?;
        let metadata = fs::symlink_metadata(&root).ok()?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return None;
        }
        Some(Self {
            root,
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
        let path = self.safe_path(relative)?;
        if !fs::symlink_metadata(&path).ok()?.is_dir() {
            return None;
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(path).ok()? {
            self.time()?;
            self.entries = self.entries.checked_add(1)?;
            if self.entries > MAX_ENTRIES {
                return None;
            }
            let entry = entry.ok()?;
            names.push(entry.file_name().into_string().ok()?);
        }
        names.sort();
        Some(names)
    }

    fn safe_path(&self, relative: &Path) -> Option<PathBuf> {
        let mut path = self.root.clone();
        for component in relative.components() {
            let std::path::Component::Normal(name) = component else {
                return None;
            };
            path.push(name);
            let metadata = fs::symlink_metadata(&path).ok()?;
            if metadata.file_type().is_symlink()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.dev() != self.device
            {
                return None;
            }
        }
        Some(path)
    }

    fn read_file(
        &mut self,
        relative: &str,
        kind: SeedFileKind,
        keep_bytes: bool,
    ) -> Option<(SeedFile, Vec<u8>)> {
        self.time()?;
        let path = self.safe_path(Path::new(relative))?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
            return None;
        }
        self.files = self.files.checked_add(1)?;
        self.bytes = self.bytes.checked_add(metadata.len())?;
        if self.files > MAX_FILES
            || self.bytes > MAX_BYTES
            || (keep_bytes && metadata.len() > MAX_DEP as u64)
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

    /// Classify each fingerprint directory independently. Unknown entries make
    /// that directory unusable; a run-only directory is never offered.
    pub fn units(&mut self, profile: &str, source: &VerifiedSource) -> Option<Vec<SeedUnit>> {
        if profile != "debug" && profile != "release" {
            return None;
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
            let build = "build-script-build";
            let has_lib = entries.iter().any(|name| name == &lib);
            let has_build = entries.iter().any(|name| name == build);
            if has_lib == has_build {
                continue;
            }
            let stem = if has_lib { lib.as_str() } else { build };
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
            let role = if has_build {
                SeedRole::BuildScriptCompile
            } else if source.proc_macro {
                SeedRole::ProcMacro
            } else {
                SeedRole::Lib
            };
            let output_dir = if has_build {
                format!("{profile}/build/{name}")
            } else {
                format!("{profile}/deps")
            };
            let dep_stem = if has_build {
                "build-script-build"
            } else {
                target.as_str()
            };
            let dep = format!("{output_dir}/{dep_stem}-{hash}.d");
            let mut artifacts = Vec::new();
            if has_build {
                artifacts.push(format!("{output_dir}/build-script-build-{hash}"));
            } else {
                let extensions: &[&str] = if source.proc_macro {
                    &["dylib", "so"]
                } else {
                    &["rmeta", "rlib"]
                };
                for extension in extensions {
                    let path = format!("{output_dir}/lib{target}-{hash}.{extension}");
                    if self.safe_path(Path::new(&path)).is_some() {
                        artifacts.push(path);
                    }
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
            if has_build {
                let uplift = format!("{output_dir}/build-script-build");
                let Some((file, _)) = self.read_file(&uplift, SeedFileKind::Uplift, false) else {
                    continue;
                };
                let Some(hashed) = files
                    .iter()
                    .find(|item| item.kind == SeedFileKind::Artifact)
                else {
                    continue;
                };
                if file.digest != hashed.digest || file.size != hashed.size {
                    continue;
                }
                files.push(file);
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
            if units.len() > 3 {
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
}
