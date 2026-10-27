//! Bounded Cargo configuration context for cold-slot seeding.
//!
//! Collection is read-only. The rendered effective values, rather than file
//! names, enter the digest; the file list is retained as review evidence.

use serde_json::{Map, Value};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 16;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_RENDERED: usize = 128 * 1024;
const FLAGS: &[&str] = &[
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_BUILD_RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTDOCFLAGS",
];
const UNSUPPORTED: &[&str] = &[
    "CARGO_BUILD_RUSTC",
    "RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
];

pub(crate) struct Context {
    pub digest: String,
    pub flags_digest: String,
    pub config_files: Vec<String>,
}

/// Facts observed in the same invocation context as the eventual Cargo child.
/// The caller supplies a verified Cargo release, canonical Cargo home, and one
/// resolved target after rejecting unsupported arguments; no probe runs here.
pub(crate) struct ProducerFacts<'a> {
    pub cargo_version: &'a str,
    pub rustc_version: &'a str,
    pub rustc: Option<&'a OsStr>,
    pub target: &'a str,
    pub recipe: &'a str,
    pub backend: &'a str,
    pub cargo_home: &'a Path,
    pub command: &'a str,
}

/// Assemble only the pinned, supported producer identity. This is pure: the
/// caller can collect configuration and probe the tools before taking seed
/// admission, and a rejected identity never creates a sidecar.
pub(crate) fn producer(
    facts: ProducerFacts<'_>,
    context: Context,
) -> Result<oxide_storage::lease::managed::SeedProducer, &'static str> {
    use oxide_storage::lease::managed::SeedProducer;
    const PREFIX: &str = "cargo 1.97.1 (c980f4866 ";
    const ADAPTER: &str = "cargo-1.97.1/c980f4866/build-dir-v1";
    if !facts.cargo_version.starts_with(PREFIX) || facts.rustc_version.trim().is_empty() {
        return Err("unsupported Cargo or compiler");
    }
    let family = match facts.command {
        "check" => "check",
        "build" | "test" | "run" | "bench" => "build",
        _ => return Err("unsupported command"),
    };
    if facts.backend != "directory-budget-v1" {
        return Err("seeding not yet supported on a removable pool");
    }
    let rustc = match facts.rustc {
        Some(value) => value.to_str().ok_or("non-UTF-8 compiler")?,
        None => "rustc",
    };
    let home = facts.cargo_home.to_str().ok_or("non-UTF-8 Cargo home")?;
    if !facts.cargo_home.is_absolute()
        || [facts.cargo_version, rustc, facts.target, facts.recipe, home]
            .iter()
            .any(|value| value.is_empty() || value.len() > 512 || value.contains('\0'))
        || facts.target.ends_with(".json")
    {
        return Err("unsupported producer identity");
    }
    Ok(SeedProducer {
        cargo_version: facts.cargo_version.to_owned(),
        rustc_digest: hash(facts.rustc_version.as_bytes()),
        rustc: rustc.to_owned(),
        target: facts.target.to_owned(),
        recipe: facts.recipe.to_owned(),
        backend: facts.backend.to_owned(),
        adapter: ADAPTER.to_owned(),
        context_digest: context.digest,
        config_files: context.config_files,
        flags_digest: context.flags_digest,
        cargo_home: home.to_owned(),
        command_family: family.to_owned(),
        command_class: facts.command.to_owned(),
    })
}

fn hash(bytes: &[u8]) -> String {
    oxide_storage::lease::seed::catalog_digest(bytes)
}

fn merge(low: &mut Value, high: Value) {
    match (low, high) {
        (Value::Object(low), Value::Object(high)) => {
            for (key, value) in high {
                match low.get_mut(&key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        low.insert(key, value);
                    }
                }
            }
        }
        // Cargo appends higher-precedence entries after lower-precedence entries.
        (Value::Array(low), Value::Array(mut high)) => low.append(&mut high),
        (low, high) => *low = high,
    }
}

fn check_config(value: &mut Value, base: &Path) -> Result<(), &'static str> {
    let table = value.as_object_mut().ok_or("unsupported config")?;
    if table
        .keys()
        .any(|key| !matches!(key.as_str(), "build" | "profile" | "target" | "env" | "net"))
    {
        return Err("unsupported config");
    }
    if let Some(build) = table.get("build") {
        let build = build.as_object().ok_or("unsupported config")?;
        if ["rustc", "rustc-wrapper", "rustc-workspace-wrapper"]
            .iter()
            .any(|key| build.contains_key(*key))
        {
            return Err("unsupported compiler or wrapper");
        }
    }
    if let Some(env) = table.get_mut("env") {
        let env = env.as_object_mut().ok_or("unsupported config")?;
        for entry in env.values_mut() {
            if let Value::Object(options) = entry {
                if options
                    .keys()
                    .any(|key| !matches!(key.as_str(), "value" | "force" | "relative"))
                {
                    return Err("unsupported env config");
                }
                let relative = match options.get("relative") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    _ => return Err("unsupported env config"),
                };
                if let Some(force) = options.get("force") {
                    if !force.is_boolean() {
                        return Err("unsupported env config");
                    }
                }
                let raw = options
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or("unsupported env config")?;
                if relative {
                    let path =
                        fs::canonicalize(base.join(raw)).map_err(|_| "unresolved env path")?;
                    let path = path.to_str().ok_or("non-UTF-8 env path")?.to_owned();
                    options.insert("value".into(), Value::String(path));
                    options.insert("relative".into(), Value::Bool(false));
                }
            } else if !entry.is_string() {
                return Err("unsupported env config");
            }
        }
    }
    Ok(())
}

fn read_config(path: &Path, total: &mut usize) -> Result<Option<Value>, &'static str> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("unreadable config"),
    };
    let metadata = file.metadata().map_err(|_| "unreadable config")?;
    if !metadata.is_file() {
        return Err("unreadable config");
    }
    let remaining = MAX_BYTES.checked_sub(*total).ok_or("config cap")?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take((remaining + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable config")?;
    if bytes.len() > remaining {
        return Err("config cap");
    }
    *total += bytes.len();
    let text = std::str::from_utf8(&bytes).map_err(|_| "invalid config")?;
    let value = basic_toml::from_str(text).map_err(|_| "invalid config")?;
    Ok(Some(value))
}

/// Collect configs in Cargo precedence order, then merge low to high. The
/// caller supplies the inherited environment and actual Cargo working dir.
pub(crate) fn collect(
    cwd: &Path,
    cargo_home: &Path,
    args: &[OsString],
    environment: &[(OsString, OsString)],
) -> Result<Context, &'static str> {
    if args
        .iter()
        .any(|arg| arg == OsStr::new("--config") || arg.as_bytes().starts_with(b"--config="))
    {
        return Err("unsupported config argument");
    }
    for (name, value) in environment {
        let name = name.to_str().ok_or("non-UTF-8 environment name")?;
        if UNSUPPORTED.contains(&name)
            || name.starts_with("CARGO_PROFILE_")
            || name.starts_with("CARGO_TARGET_")
            || (name == "OXIDE_ALLOW_RUSTC_WRAPPER" && value == OsStr::new("1"))
        {
            return Err("unsupported environment");
        }
    }
    let cwd = fs::canonicalize(cwd).map_err(|_| "unresolved working directory")?;
    let home = fs::canonicalize(cargo_home).map_err(|_| "unresolved Cargo home")?;
    let mut dirs: Vec<(PathBuf, PathBuf)> = cwd
        .ancestors()
        .map(|base| (base.join(".cargo"), base.to_path_buf()))
        .collect();
    let home_base = home.parent().ok_or("unresolved Cargo home")?.to_path_buf();
    dirs.push((home, home_base));
    let mut files = Vec::new();
    let mut seen_dirs = std::collections::HashSet::new();
    let mut total = 0;
    for (dir, base) in dirs {
        if !seen_dirs.insert(dir.clone()) {
            continue;
        }
        match fs::symlink_metadata(&dir) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err("unsafe config directory")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("unreadable config directory"),
        }
        // Cargo prefers config over config.toml in the same directory.
        let mut preferred_present = false;
        for name in ["config", "config.toml"] {
            let path = dir.join(name);
            if let Some(mut value) = read_config(&path, &mut total)? {
                check_config(&mut value, &base)?;
                if files.len() == MAX_FILES {
                    return Err("config cap");
                }
                let text = path.to_str().ok_or("non-UTF-8 config path")?.to_owned();
                let active = !preferred_present;
                preferred_present = true;
                files.push((text, value, active));
            }
        }
    }
    let mut effective = Value::Object(Map::new());
    for (_, value, active) in files.iter().rev() {
        if *active {
            merge(&mut effective, value.clone());
        }
    }
    let mut flags = Vec::new();
    let mut context = serde_json::to_vec(&effective).map_err(|_| "invalid config")?;
    if context.len() > MAX_RENDERED {
        return Err("config cap");
    }
    for name in FLAGS.iter().copied().chain(std::iter::once("RUSTC")) {
        context.extend_from_slice(name.as_bytes());
        context.push(0);
        let value = environment
            .iter()
            .find(|(key, _)| key == OsStr::new(name))
            .map(|(_, value)| value);
        match value {
            Some(value) => {
                context.push(1);
                context.extend_from_slice(hash(value.as_bytes()).as_bytes());
            }
            None => context.push(0),
        }
        if name != "RUSTC" {
            flags.extend_from_slice(
                &context[context.len() - if value.is_some() { 65 } else { 1 }..],
            );
        }
    }
    Ok(Context {
        digest: hash(&context),
        flags_digest: hash(&flags),
        config_files: files.into_iter().map(|(path, _, _)| path).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> Context {
        Context {
            digest: hash(b"context"),
            flags_digest: hash(b"flags"),
            config_files: vec![],
        }
    }

    fn facts() -> ProducerFacts<'static> {
        ProducerFacts {
            cargo_version: "cargo 1.97.1 (c980f4866 2026-01-01)",
            rustc_version: "rustc 1.97.1\nhost: example-host\n",
            rustc: None,
            target: "example-host",
            recipe: "quick:dev-recipe-2",
            backend: "directory-budget-v1",
            cargo_home: Path::new("/fixture/cargo"),
            command: "check",
        }
    }

    #[test]
    fn producer_binds_facts_and_refuses_unsupported_inputs() {
        let recorded = producer(facts(), test_context()).unwrap();
        assert_eq!(recorded.adapter, "cargo-1.97.1/c980f4866/build-dir-v1");
        assert_eq!(
            recorded.rustc_digest,
            hash(facts().rustc_version.as_bytes())
        );
        assert_eq!(recorded.command_family, "check");
        assert_eq!(recorded.cargo_home, "/fixture/cargo");

        let mut input = facts();
        input.cargo_version = "cargo 1.97.1 (wrong 2026-01-01)";
        assert!(producer(input, test_context()).is_err());
        let mut input = facts();
        input.command = "exec";
        assert!(producer(input, test_context()).is_err());
        let mut input = facts();
        input.backend = "apfs-volume-v1";
        assert!(producer(input, test_context()).is_err());
        let mut input = facts();
        input.target = "/fixture/custom.json";
        assert!(producer(input, test_context()).is_err());
        let mut input = facts();
        input.cargo_home = Path::new("relative");
        assert!(producer(input, test_context()).is_err());
    }

    #[test]
    fn merge_uses_higher_precedence_and_keeps_unrelated_values() {
        let mut low: Value =
            basic_toml::from_str("[build]\njobs = 1\nrustflags = ['low']\n[net]\noffline = true\n")
                .unwrap();
        let high: Value =
            basic_toml::from_str("[build]\njobs = 2\nrustflags = ['high']\n").unwrap();
        merge(&mut low, high);
        assert_eq!(low["build"]["jobs"], 2);
        assert_eq!(
            low["build"]["rustflags"],
            serde_json::json!(["low", "high"])
        );
        assert_eq!(low["net"]["offline"], true);
    }

    #[test]
    fn rejects_unmodelled_configuration_and_compiler() {
        let mut source: Value =
            basic_toml::from_str("[source.crates-io]\nreplace-with = 'vendor'\n").unwrap();
        assert!(check_config(&mut source, Path::new("/")).is_err());
        let mut compiler: Value = basic_toml::from_str("[build]\nrustc = 'other'\n").unwrap();
        assert!(check_config(&mut compiler, Path::new("/")).is_err());
    }
}
