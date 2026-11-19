//! Effective Cargo incremental setting for slot compatibility.
use crate::{cargo_home, cli, wrapper};
use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// An empty discriminator preserves the pre-change recipe-default key.
/// Unknown settings share their own key, separate from either known value.
pub fn key_component(checkout: &Path, invocation: &cli::Invocation) -> Result<String, String> {
    let setting = resolve(checkout, invocation);
    if let Err(reason) = &setting {
        eprintln!("oxide: incremental setting unknown ({reason}); using an unknown-setting slot");
    }
    Ok(component(invocation.profile, setting.as_ref().ok().copied()).to_owned())
}

fn component(profile: cli::Profile, setting: Option<bool>) -> &'static str {
    match setting {
        Some(value) if value == (profile != cli::Profile::Release) => "",
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn env_bool(name: &str, numeric: bool) -> Result<Option<bool>, String> {
    std::env::var_os(name)
        .map(|value| parse_env(&value, numeric))
        .transpose()
        .map_err(|()| format!("{name} is malformed"))
}

fn parse_env(value: &OsStr, numeric: bool) -> Result<bool, ()> {
    match value.to_str() {
        Some("1") if numeric => Ok(true),
        Some("0") if numeric => Ok(false),
        Some("true") if !numeric => Ok(true),
        Some("false") if !numeric => Ok(false),
        _ => Err(()),
    }
}

fn field(doc: &Value, path: &[&str]) -> Result<Option<bool>, String> {
    let mut current = doc;
    for key in path {
        if !current.is_object() {
            return Err(format!("{} is not a table", path.join(".")));
        }
        match current.get(*key) {
            Some(value) => current = value,
            None => return Ok(None),
        }
    }
    current
        .as_bool()
        .map(Some)
        .ok_or_else(|| format!("{} is not a boolean", path.join(".")))
}

fn layer(docs: &[Value], path: &[&str]) -> Result<Option<bool>, String> {
    for doc in docs {
        if let Some(value) = field(doc, path)? {
            return Ok(Some(value));
        }
        if doc.get("include").is_some() {
            return Err("a Cargo config includes another file".into());
        }
    }
    Ok(None)
}

fn global_override(
    cli_docs: &[Value],
    env: Result<Option<bool>, String>,
) -> Result<Option<bool>, String> {
    if let Some(value) = layer(cli_docs, &["build", "incremental"])? {
        return Ok(Some(value));
    }
    env
}

fn top_override(
    value: Result<Option<bool>, String>,
    lower: impl FnOnce() -> Result<bool, String>,
) -> Result<bool, String> {
    match value? {
        Some(value) => Ok(value),
        None => lower(),
    }
}

fn resolve(checkout: &Path, invocation: &cli::Invocation) -> Result<bool, String> {
    top_override(env_bool("CARGO_INCREMENTAL", true), || {
        resolve_lower(checkout, invocation)
    })
}

fn resolve_lower(checkout: &Path, invocation: &cli::Invocation) -> Result<bool, String> {
    // An external tool may supply its own --config and profile.
    if !cli::reads_config_args(&invocation.command) {
        return Err("external command chooses its Cargo configuration".into());
    }
    let args = &invocation.cargo_args[..];
    let mut cli_docs = Vec::new();
    for config in wrapper::config_values(args).iter().rev() {
        cli_docs
            .push(basic_toml::from_str(config).map_err(|_| "cannot parse --config".to_owned())?);
    }
    if let Some(value) = global_override(&cli_docs, env_bool("CARGO_BUILD_INCREMENTAL", false))? {
        return Ok(value);
    }
    let cwd = std::env::current_dir().map_err(|_| "cannot read cwd".to_owned())?;
    let home = cargo_home::resolve()?;
    let files = wrapper::config_files(&cwd, &home);
    // The wrapper's diagnostic discovery skips files it cannot open. A
    // skipped layer is not evidence that it cannot affect this key.
    for dir in cwd
        .ancestors()
        .map(|dir| dir.join(".cargo"))
        .chain(std::iter::once(home))
    {
        let plain = dir.join("config");
        let toml = dir.join("config.toml");
        for path in [&plain, &toml] {
            match std::fs::symlink_metadata(path) {
                Ok(_)
                    if !names_config(&files, path)
                        && !(path == &toml && names_config(&files, &plain)) =>
                {
                    return Err(format!(
                        "Cargo config {} cannot be classified",
                        path.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(format!(
                        "Cargo config {} cannot be inspected",
                        path.display()
                    ))
                }
            }
        }
    }
    let mut docs = Vec::new();
    for path in files {
        docs.push(cargo_home::read_config(&path)?);
    }
    if let Some(value) = layer(&docs, &["build", "incremental"])? {
        return Ok(value);
    }
    let profile = if invocation.profile == cli::Profile::Release {
        "release"
    } else if invocation.command == "bench" && !invocation.explicit_profile {
        "bench"
    } else if invocation.command == "test" {
        "test"
    } else {
        "dev"
    };
    let base = match profile {
        "test" => "dev",
        "bench" => "release",
        other => other,
    };
    let manifest = cargo_home::read_config(&checkout.join("Cargo.toml"))?;
    for name in [profile, base] {
        let env = format!("CARGO_PROFILE_{}_INCREMENTAL", name.to_ascii_uppercase());
        if let Some(value) = layer(&cli_docs, &["profile", name, "incremental"])? {
            return Ok(value);
        }
        if let Some(value) = env_bool(&env, false)? {
            return Ok(value);
        }
        if let Some(value) = layer(&docs, &["profile", name, "incremental"])? {
            return Ok(value);
        }
        if let Some(value) = field(&manifest, &["profile", name, "incremental"])? {
            return Ok(value);
        }
        if name == base {
            break;
        }
    }
    Ok(base == "dev")
}

/// Whether `path` is one of `files`, compared canonically. The same Cargo
/// config reached through an ancestor `.cargo` and `CARGO_HOME` (a link, a
/// trailing slash, another spelling) is one file, not an unclassified one.
fn names_config(files: &[PathBuf], path: &Path) -> bool {
    if files.iter().any(|file| file == path) {
        return true;
    }
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    files
        .iter()
        .any(|file| std::fs::canonicalize(file).is_ok_and(|file| file == canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a `.cargo/config.toml` reached through a link (an ancestor
    /// `.cargo` and `CARGO_HOME` naming one directory differently) was refused as
    /// unclassified, so an ordinary build failed for no config reason.
    #[test]
    fn one_config_reached_by_two_spellings_is_one_file() {
        let root = std::env::temp_dir().join(format!("oxide-names-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real/.cargo");
        std::fs::create_dir_all(&real).expect("fixture");
        std::fs::write(real.join("config.toml"), "").expect("config");
        let link = root.join("link");
        std::os::unix::fs::symlink(root.join("real"), &link).expect("link");
        let files = vec![real.join("config.toml")];
        assert!(names_config(&files, &link.join(".cargo/config.toml")));
        assert!(!names_config(
            &files,
            &root.join("absent/.cargo/config.toml")
        ));
        std::fs::remove_dir_all(&root).expect("remove own fixture");
    }

    #[test]
    fn component_preserves_defaults_and_partitions_unknown() {
        for profile in [cli::Profile::Quick, cli::Profile::Debug] {
            assert_eq!(component(profile, Some(true)), "");
            assert_eq!(component(profile, Some(false)), "false");
            assert_eq!(component(profile, None), "unknown");
            assert_eq!(component(profile, None), component(profile, None));
        }
        assert_eq!(component(cli::Profile::Release, Some(false)), "");
        assert_eq!(component(cli::Profile::Release, Some(true)), "true");
        assert_eq!(component(cli::Profile::Release, None), "unknown");
    }

    #[test]
    fn global_incremental_precedes_build_config_and_profile() {
        use std::ffi::OsString;
        use std::fs;
        use std::process::Command;

        if let Some(case) = std::env::var_os("OXIDE_TEST_INCREMENTAL_CASE") {
            let root = std::env::current_dir().unwrap();
            let args = if case == "profile" {
                Vec::new()
            } else {
                vec![
                    OsString::from("--config"),
                    OsString::from("build.incremental=false"),
                ]
            };
            let invocation = cli::parse_leased("build", &args).unwrap();
            let expected = case == "env-on" || case == "profile";
            assert_eq!(resolve(&root, &invocation).unwrap(), expected);
            return;
        }

        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "incremental-{}",
            crate::prune::fresh_nonce().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let home = root.join("cargo-home");
        fs::create_dir(&home).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[profile.dev]\nincremental = true\n",
        )
        .unwrap();
        for case in ["env-on", "env-off", "config", "profile"] {
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "incremental::tests::global_incremental_precedes_build_config_and_profile",
                    "--test-threads=1",
                ])
                .current_dir(&root)
                .env_clear()
                .env("HOME", &root)
                .env("CARGO_HOME", &home)
                .env("OXIDE_TEST_INCREMENTAL_CASE", case);
            if case == "env-on" {
                child.env("CARGO_INCREMENTAL", "1");
            }
            if case == "env-off" {
                child.env("CARGO_INCREMENTAL", "0");
            }
            let status = child.status().unwrap();
            assert!(status.success(), "precedence case {case}: {status}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn configuration_layers_and_malformed_values() {
        let docs = [
            basic_toml::from_str::<Value>("[build]\nincremental = false").unwrap(),
            basic_toml::from_str::<Value>("[build]\nincremental = true").unwrap(),
        ];
        assert_eq!(
            layer(&docs, &["build", "incremental"]).unwrap(),
            Some(false)
        );
        assert_eq!(global_override(&docs, Ok(Some(true))).unwrap(), Some(false));
        assert_eq!(global_override(&[], Ok(Some(true))).unwrap(), Some(true));
        assert_eq!(global_override(&[], Ok(None)).unwrap(), None);
        assert!(!top_override(Ok(Some(false)), || Ok(true)).unwrap());
        assert!(top_override(Ok(None), || Ok(true)).unwrap());
        assert!(top_override(Err("malformed".into()), || Ok(true)).is_err());
        assert_eq!(parse_env(OsStr::new("1"), true), Ok(true));
        assert_eq!(parse_env(OsStr::new("0"), true), Ok(false));
        assert!(parse_env(OsStr::new("maybe"), true).is_err());
        let malformed = basic_toml::from_str::<Value>("[build]\nincremental = 'yes'").unwrap();
        assert!(layer(&[malformed], &["build", "incremental"]).is_err());
    }
}
