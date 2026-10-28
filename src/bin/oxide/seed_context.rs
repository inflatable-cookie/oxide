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

    /// A private synthetic account tree: `home/cargo-home` is Cargo home,
    /// `home` is the account home and `home/work/ws/member*` are members.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new() -> Self {
            let template = std::env::temp_dir().join("oxide-seed-context.XXXXXX");
            let mut name = std::ffi::CString::new(template.as_os_str().as_bytes())
                .unwrap()
                .into_bytes_with_nul();
            // SAFETY: name is writable, NUL-terminated and ends in XXXXXX.
            assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
            let name = std::ffi::CStr::from_bytes_until_nul(&name).unwrap();
            let root = fs::canonicalize(PathBuf::from(OsStr::from_bytes(name.to_bytes()))).unwrap();
            let tree = Self { root };
            tree.dirs(&[
                "home/cargo-home",
                "home/work/ws/member",
                "home/work/ws/member2",
                "home/other",
            ]);
            tree
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }

        fn dirs(&self, relative: &[&str]) {
            for dir in relative {
                fs::create_dir_all(self.path(dir)).unwrap();
            }
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.path(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn context(
            &self,
            cwd: &str,
            environment: &[(&str, &str)],
        ) -> Result<Context, &'static str> {
            let environment: Vec<(OsString, OsString)> = environment
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value)))
                .collect();
            collect(
                &self.path(cwd),
                &self.path("home/cargo-home"),
                &[],
                &environment,
            )
        }

        fn digest(&self, cwd: &str) -> Result<String, &'static str> {
            self.context(cwd, &[]).map(|context| context.digest)
        }

        fn files(&self, relative: &[&str]) -> Vec<String> {
            relative
                .iter()
                .map(|path| self.path(path).to_str().unwrap().to_owned())
                .collect()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn member_and_above_home_configs_merge_by_cargo_precedence() {
        let tree = Tree::new();
        tree.write(".cargo/config.toml", "[build]\njobs = 1\n");
        tree.write("home/cargo-home/config.toml", "[build]\njobs = 3\n");
        tree.write("home/work/ws/.cargo/config.toml", "[build]\njobs = 2\n");
        let member = tree.context("home/work/ws/member", &[]).unwrap();
        // Highest precedence first: the working directory's ancestor, the
        // ancestor above the account home, then Cargo home.
        assert_eq!(
            member.config_files,
            tree.files(&[
                "home/work/ws/.cargo/config.toml",
                ".cargo/config.toml",
                "home/cargo-home/config.toml",
            ])
        );
        // Only the winning value enters the digest: a tree that carries just
        // the member's ancestor config renders the same effective value.
        let only_ws = Tree::new();
        only_ws.write("home/work/ws/.cargo/config.toml", "[build]\njobs = 2\n");
        assert_eq!(
            member.digest,
            only_ws.digest("home/work/ws/member").unwrap()
        );
        // A sibling member under the same ancestor sees the same context.
        assert_eq!(member.digest, tree.digest("home/work/ws/member2").unwrap());
        // Remove the ancestor config: the ancestor above the account home now
        // supplies the value, as Cargo's walk does.
        fs::remove_file(tree.path("home/work/ws/.cargo/config.toml")).unwrap();
        let only_root = Tree::new();
        only_root.write(".cargo/config.toml", "[build]\njobs = 1\n");
        assert_eq!(
            tree.digest("home/work/ws/member").unwrap(),
            only_root.digest("home/work/ws/member").unwrap()
        );
        assert_ne!(member.digest, tree.digest("home/work/ws/member").unwrap());
    }

    #[test]
    fn config_and_config_toml_in_one_directory_follow_cargo_pair_precedence() {
        let tree = Tree::new();
        tree.write("home/work/ws/.cargo/config", "[build]\njobs = 2\n");
        tree.write("home/work/ws/.cargo/config.toml", "[build]\njobs = 9\n");
        let both = tree.context("home/work/ws/member", &[]).unwrap();
        // `config` is active and `config.toml` is recorded but inert.
        assert_eq!(
            both.config_files,
            tree.files(&[
                "home/work/ws/.cargo/config",
                "home/work/ws/.cargo/config.toml",
            ])
        );
        let config_only = {
            fs::remove_file(tree.path("home/work/ws/.cargo/config.toml")).unwrap();
            tree.digest("home/work/ws/member").unwrap()
        };
        assert_eq!(both.digest, config_only);
        // With `config` removed, `config.toml` becomes the active file.
        fs::remove_file(tree.path("home/work/ws/.cargo/config")).unwrap();
        tree.write("home/work/ws/.cargo/config.toml", "[build]\njobs = 9\n");
        assert_ne!(both.digest, tree.digest("home/work/ws/member").unwrap());
    }

    #[test]
    fn relative_env_values_resolve_against_their_own_config_base() {
        let tree = Tree::new();
        tree.dirs(&[
            "tools",
            "home/tools",
            "home/work/ws/tools",
            "home/work/ws/other",
        ]);
        // The ancestor above the account home resolves against its own
        // directory; Cargo home's values resolve against the parent of Cargo
        // home, which is the account home.
        tree.write(
            ".cargo/config.toml",
            "[env]\nTOOLS = { value = 'tools', relative = true }\n",
        );
        tree.write(
            "home/cargo-home/config.toml",
            "[env]\nHOME_TOOLS = { value = 'tools', relative = true }\n",
        );
        let root_value = tree.digest("home/other").unwrap();
        tree.write(
            "home/work/ws/.cargo/config.toml",
            "[env]\nTOOLS = { value = 'tools', relative = true }\n",
        );
        let ws_value = tree.digest("home/work/ws/member").unwrap();
        // Two working directories under the same ancestor resolve the same
        // relative value to the same absolute path.
        assert_eq!(ws_value, tree.digest("home/work/ws/member2").unwrap());
        // The member's ws/tools differs from the root's tools.
        assert_ne!(root_value, ws_value);
        // A differing relative value changes the digest.
        tree.write(
            "home/work/ws/.cargo/config.toml",
            "[env]\nTOOLS = { value = 'other', relative = true }\n",
        );
        assert_ne!(ws_value, tree.digest("home/work/ws/member").unwrap());
        // An unresolvable Cargo-home relative value refuses the whole context.
        fs::remove_dir(tree.path("home/tools")).unwrap();
        assert_eq!(tree.digest("home/other"), Err("unresolved env path"));
    }

    #[test]
    fn unresolved_relative_env_and_unsupported_env_shapes_refuse() {
        let cases = [
            (
                "[env]\nFOO = { value = 'missing', relative = true }\n",
                "unresolved env path",
            ),
            (
                "[env]\nFOO = { value = 'x', extra = 1 }\n",
                "unsupported env config",
            ),
            (
                "[env]\nFOO = { value = 'x', force = 'yes' }\n",
                "unsupported env config",
            ),
            (
                "[env]\nFOO = { value = 'x', relative = 'yes' }\n",
                "unsupported env config",
            ),
            (
                "[env]\nFOO = { relative = true }\n",
                "unsupported env config",
            ),
            ("[env]\nFOO = 1\n", "unsupported env config"),
            ("[env]\nFOO = [1]\n", "unsupported env config"),
        ];
        for (text, reason) in cases {
            let tree = Tree::new();
            tree.write("home/work/ws/.cargo/config.toml", text);
            assert_eq!(tree.digest("home/work/ws/member"), Err(reason), "{text}");
        }
    }

    #[test]
    fn unsupported_tables_and_compiler_or_wrapper_keys_refuse() {
        let cases = [
            "[source.crates-io]\nreplace-with = 'vendor'\n",
            "[patch.crates-io]\nfoo = { path = 'x' }\n",
            "[registries.internal]\nindex = 'sparse+https://example.invalid/'\n",
            "[unstable]\nbuild-std = []\n",
            "[alias]\nb = 'build'\n",
            "[build]\nrustc = 'other'\n",
            "[build]\nrustc-wrapper = 'sccache'\n",
            "[build]\nrustc-workspace-wrapper = 'sccache'\n",
        ];
        for text in cases {
            let tree = Tree::new();
            tree.write("home/work/ws/.cargo/config.toml", text);
            assert!(tree.digest("home/work/ws/member").is_err(), "{text}");
        }
        // The modelled tables stay accepted.
        let tree = Tree::new();
        tree.write(
            "home/work/ws/.cargo/config.toml",
            "[build]\njobs = 2\n[profile.dev]\ndebug = false\n[target.x86_64-apple-darwin]\nlinker = 'cc'\n[net]\noffline = true\n",
        );
        assert!(tree.digest("home/work/ws/member").is_ok());
    }

    #[test]
    fn unreadable_unparseable_and_non_utf8_configs_refuse() {
        let tree = Tree::new();
        tree.write("home/work/ws/.cargo/config.toml", "[build\n");
        assert_eq!(tree.digest("home/work/ws/member"), Err("invalid config"));

        let tree = Tree::new();
        fs::create_dir_all(tree.path("home/work/ws/.cargo/config.toml")).unwrap();
        assert_eq!(tree.digest("home/work/ws/member"), Err("unreadable config"));

        let tree = Tree::new();
        tree.write("home/work/ws/real.toml", "[build]\njobs = 2\n");
        fs::create_dir_all(tree.path("home/work/ws/.cargo")).unwrap();
        std::os::unix::fs::symlink(
            tree.path("home/work/ws/real.toml"),
            tree.path("home/work/ws/.cargo/config.toml"),
        )
        .unwrap();
        assert_eq!(tree.digest("home/work/ws/member"), Err("unreadable config"));

        let tree = Tree::new();
        fs::create_dir_all(tree.path("home/work/ws/.cargo")).unwrap();
        fs::write(tree.path("home/work/ws/.cargo/config"), [0xff, 0xfe, b'\n']).unwrap();
        assert_eq!(tree.digest("home/work/ws/member"), Err("invalid config"));

        let tree = Tree::new();
        tree.dirs(&["home/work/ws/elsewhere/.cargo"]);
        std::os::unix::fs::symlink(
            tree.path("home/work/ws/elsewhere/.cargo"),
            tree.path("home/work/ws/.cargo"),
        )
        .unwrap();
        assert_eq!(
            tree.digest("home/work/ws/member"),
            Err("unsafe config directory")
        );
    }

    #[test]
    fn config_file_and_rendered_caps_refuse_without_partial_context() {
        // Sixteen ancestor configs are the most the walk may collect. The
        // chain stays inside the synthetic tree; the walk above it only reads.
        let tree = Tree::new();
        let mut chain = String::from("chain");
        for level in 0..16 {
            chain.push_str(&format!("/l{level}"));
            tree.write(
                &format!("{chain}/.cargo/config.toml"),
                "[build]\njobs = 2\n",
            );
        }
        assert!(tree.digest(&chain).is_ok());
        tree.write(&format!("{chain}/.cargo/config"), "[build]\njobs = 3\n");
        assert_eq!(tree.digest(&chain), Err("config cap"));

        // A file one byte past the four MiB byte budget is refused unparsed.
        let tree = Tree::new();
        let mut text = String::from("#");
        text.push_str(&"x".repeat(MAX_BYTES));
        tree.write("home/work/ws/.cargo/config.toml", &text);
        assert_eq!(tree.digest("home/work/ws/member"), Err("config cap"));

        // Rendered records over 128 KiB refuse even when every file fits.
        let tree = Tree::new();
        let flags = vec![format!("'{}'", "f".repeat(120)); 1200].join(", ");
        tree.write(
            "home/work/ws/.cargo/config.toml",
            &format!("[build]\nrustflags = [{flags}]\n"),
        );
        assert_eq!(tree.digest("home/work/ws/member"), Err("config cap"));
    }

    #[test]
    fn unsupported_arguments_and_environment_refuse_and_digests_track_values() {
        let tree = Tree::new();
        let cwd = tree.path("home/work/ws/member");
        let home = tree.path("home/cargo-home");
        for argument in ["--config", "--config=build.jobs=1"] {
            let refused = collect(&cwd, &home, &[OsString::from(argument)], &[]);
            assert_eq!(
                refused.err(),
                Some("unsupported config argument"),
                "{argument}"
            );
        }
        let refusals = [
            "CARGO_BUILD_RUSTC",
            "RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            "CARGO_PROFILE_DEV_OPT_LEVEL",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
        ];
        for name in refusals {
            assert!(
                tree.context("home/work/ws/member", &[(name, "x")]).is_err(),
                "{name}"
            );
        }
        assert!(tree
            .context("home/work/ws/member", &[("OXIDE_ALLOW_RUSTC_WRAPPER", "1")])
            .is_err());
        assert!(tree
            .context("home/work/ws/member", &[("OXIDE_ALLOW_RUSTC_WRAPPER", "0")])
            .is_ok());

        // Flags enter the digest by exact value and presence.
        let absent = tree.context("home/work/ws/member", &[]).unwrap();
        let first = tree
            .context("home/work/ws/member", &[("RUSTFLAGS", "-Copt-level=1")])
            .unwrap();
        let again = tree
            .context("home/work/ws/member", &[("RUSTFLAGS", "-Copt-level=1")])
            .unwrap();
        let second = tree
            .context("home/work/ws/member", &[("RUSTFLAGS", "-Cdebuginfo=0")])
            .unwrap();
        assert_eq!(first.digest, again.digest);
        assert_eq!(first.flags_digest, again.flags_digest);
        assert_ne!(first.digest, second.digest);
        assert_ne!(first.flags_digest, second.flags_digest);
        assert_ne!(absent.flags_digest, first.flags_digest);
    }
}
