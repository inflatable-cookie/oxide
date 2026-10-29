//! Effective Cargo incremental setting for slot compatibility.
use crate::{cargo_home, cli, prune, wrapper};
use serde_json::Value;
use std::ffi::OsStr;
use std::path::Path;

/// A missing discriminator preserves the pre-change recipe-default key. An
/// unknown gets a fresh key, so no prior build can be mistaken for it.
pub fn key_component(checkout: &Path, invocation: &cli::Invocation) -> Result<String, String> {
    match resolve(checkout, invocation) {
        Ok(value) if value == (invocation.profile != cli::Profile::Release) => Ok(String::new()),
        Ok(value) => Ok(value.to_string()),
        Err(reason) => {
            eprintln!("oxide: incremental setting unknown ({reason}); building cold");
            Ok(format!("unknown-{}", prune::fresh_nonce()?))
        }
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
                Ok(_) if !files.contains(path) && !(path == &toml && files.contains(&plain)) => {
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

#[cfg(test)]
mod tests {
    use super::*;
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
