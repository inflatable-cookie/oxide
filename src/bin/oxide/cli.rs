//! Command classes, Oxide profiles and argument translation.

use std::ffi::OsString;
use std::path::PathBuf;

/// How one `oxide` command reaches Cargo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    /// `check`, `test`, `clippy`, `build`, `run`: profile-aware and leased.
    Modelled,
    /// Built-ins that compile plus every unknown or external subcommand.
    Building,
    /// Built-ins that never compile; plain Cargo with no lease.
    Plain,
    /// `clean`: private outputs and slot affinity; never a slot.
    Clean,
    /// `init`: `cargo init` when no manifest exists, then adoption.
    Init,
    /// `new`: `cargo new`, then adoption.
    New,
    /// `status`: Oxide-owned.
    Status,
}

pub fn classify(command: &str) -> Class {
    match command {
        "check" | "test" | "clippy" | "build" | "run" => Class::Modelled,
        "clean" => Class::Clean,
        "status" => Class::Status,
        "init" => Class::Init,
        "new" => Class::New,
        "fmt" | "tree" | "metadata" | "add" | "remove" | "update" | "info" | "search"
        | "install" | "uninstall" | "publish" | "login" | "logout" | "owner" | "yank"
        | "vendor" | "fetch" | "generate-lockfile" | "locate-project" | "pkgid" | "version"
        | "help" | "report" | "config" | "package" | "verify-project" => Class::Plain,
        _ => Class::Building,
    }
}

/// Built-ins that compile and accept Cargo's `--profile`.
fn is_building_builtin(command: &str) -> bool {
    matches!(command, "doc" | "bench" | "rustc" | "rustdoc" | "fix")
}

/// The user's intent. Strategies stay `build` in the foundation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    Quick,
    Debug,
    Release,
}

impl Profile {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "quick" => Ok(Self::Quick),
            "debug" => Ok(Self::Debug),
            "release" => Ok(Self::Release),
            other => Err(format!(
                "--profile {other} is not an Oxide profile; use quick, debug or release"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    /// Cargo profile name; `release` stays the project's own profile.
    pub fn cargo_name(self) -> &'static str {
        match self {
            Self::Quick => "oxide-quick",
            Self::Debug => "oxide-debug",
            Self::Release => "release",
        }
    }
}

/// Environment-only profile definitions. Child Cargo calls inherit them; a
/// `--config` definition is invisible to an external subcommand's `$CARGO`.
pub const PROFILE_ENVIRONMENT: [(&str, &str); 5] = [
    ("CARGO_PROFILE_OXIDE_QUICK_INHERITS", "dev"),
    ("CARGO_PROFILE_OXIDE_QUICK_DEBUG", "line-tables-only"),
    ("CARGO_PROFILE_OXIDE_DEBUG_INHERITS", "dev"),
    ("CARGO_PROFILE_OXIDE_DEBUG_DEBUG", "full"),
    ("CARGO_PROFILE_OXIDE_DEBUG_BUILD_OVERRIDE_DEBUG", "full"),
];

/// A parsed leased command.
#[derive(Debug, Eq, PartialEq)]
pub struct Invocation {
    pub command: String,
    /// Slot profile; building passthrough uses quick unless a built-in names one.
    pub profile: Profile,
    /// Arguments after `cargo`, starting with the subcommand.
    pub cargo_args: Vec<OsString>,
    /// `run` only: arguments for the built program.
    pub program_args: Vec<OsString>,
    /// `run` only: the caller asked for JSON messages on stdout.
    pub json_messages: bool,
    pub manifest_path: Option<PathBuf>,
    pub target: Option<String>,
}

/// Parse a leased command (`Modelled` or `Building`).
pub fn parse_leased(command: &str, args: &[OsString]) -> Result<Invocation, String> {
    refuse_storage_overrides(args)?;
    let manifest_path = flag_value(args, "--manifest-path").map(PathBuf::from);
    let target = flag_value(args, "--target");
    let modelled = classify(command) == Class::Modelled;
    if !modelled && !is_building_builtin(command) {
        // External and unknown subcommands receive their arguments untouched.
        let mut cargo_args = vec![OsString::from(command)];
        cargo_args.extend_from_slice(args);
        return Ok(Invocation {
            command: command.to_owned(),
            profile: Profile::Quick,
            cargo_args,
            program_args: Vec::new(),
            json_messages: false,
            manifest_path,
            target,
        });
    }

    let run = command == "run";
    let mut selected = None;
    let mut release = false;
    let mut json_messages = false;
    let mut cargo_args = vec![OsString::from(if run { "build" } else { command })];
    let mut program_args = Vec::new();
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        let text = argument.to_str().unwrap_or_default();
        if text == "--" {
            if run {
                program_args.extend(iter.by_ref().cloned());
            } else {
                cargo_args.push(argument.clone());
                cargo_args.extend(iter.by_ref().cloned());
            }
            break;
        }
        let profile_value = if text == "--profile" {
            Some(
                iter.next()
                    .and_then(|value| value.to_str())
                    .ok_or("--profile requires quick, debug or release")?,
            )
        } else {
            text.strip_prefix("--profile=")
        };
        if let Some(value) = profile_value {
            if selected.replace(Profile::parse(value)?).is_some() {
                return Err("--profile may be given only once".to_owned());
            }
            continue;
        }
        if text == "--release" || text == "-r" {
            release = true;
            continue;
        }
        if run {
            let format = if text == "--message-format" {
                Some(
                    iter.next()
                        .and_then(|value| value.to_str())
                        .ok_or("--message-format requires a value")?,
                )
            } else {
                text.strip_prefix("--message-format=")
            };
            if let Some(format) = format {
                json_messages = format.starts_with("json");
                continue;
            }
        }
        cargo_args.push(argument.clone());
    }
    if release && selected.is_some_and(|profile| profile != Profile::Release) {
        return Err("--release conflicts with --profile quick or debug".to_owned());
    }
    let explicit = release || selected.is_some();
    let profile = if release {
        Profile::Release
    } else {
        selected.unwrap_or(Profile::Quick)
    };
    // Modelled commands always name their Cargo profile; building built-ins
    // keep Cargo's own default unless the caller chose an Oxide profile.
    if modelled || explicit {
        let position = cargo_args
            .iter()
            .position(|argument| argument == "--")
            .unwrap_or(cargo_args.len());
        let flags = if profile == Profile::Release {
            vec![OsString::from("--release")]
        } else {
            vec![
                OsString::from("--profile"),
                OsString::from(profile.cargo_name()),
            ]
        };
        cargo_args.splice(position..position, flags);
    }
    if run {
        cargo_args.push(OsString::from("--message-format=json-render-diagnostics"));
    }
    Ok(Invocation {
        command: command.to_owned(),
        profile,
        cargo_args,
        program_args,
        json_messages,
        manifest_path,
        target,
    })
}

/// Oxide owns storage placement: `--target-dir` and `--config` values that set
/// the build or target directory are refused. A `--config` file path is
/// refused because its contents could do the same.
pub fn refuse_storage_overrides(args: &[OsString]) -> Result<(), String> {
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        let text = argument.to_str().unwrap_or_default();
        if text == "--" {
            break;
        }
        if text == "--target-dir" || text.starts_with("--target-dir=") {
            return Err(
                "--target-dir is refused: Oxide places build output (intermediates in its slot, final output in <workspace>/target)"
                    .to_owned(),
            );
        }
        let config = if text == "--config" {
            Some(
                iter.next()
                    .and_then(|value| value.to_str())
                    .ok_or("--config requires a KEY=VALUE or a file path")?,
            )
        } else {
            text.strip_prefix("--config=")
        };
        let Some(config) = config else {
            continue;
        };
        let Some((key, _)) = config.split_once('=') else {
            return Err(format!(
                "--config {config} is refused: a config file could set build.build-dir or build.target-dir, which Oxide owns"
            ));
        };
        let key: String = key
            .chars()
            .filter(|character| !matches!(character, '"' | '\'' | ' ' | '\t'))
            .collect::<String>()
            .replace('_', "-");
        if matches!(
            key.as_str(),
            "build" | "build.build-dir" | "build.target-dir"
        ) {
            return Err(format!(
                "--config {key} is refused: Oxide owns build and target directory placement"
            ));
        }
    }
    Ok(())
}

/// The value of `--flag value` or `--flag=value` before any `--`.
fn flag_value(args: &[OsString], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    let prefix = format!("{flag}=");
    while let Some(argument) = iter.next() {
        let text = argument.to_str()?;
        if text == "--" {
            return None;
        }
        if text == flag {
            return iter
                .next()
                .and_then(|value| value.to_str())
                .map(str::to_owned);
        }
        if let Some(value) = text.strip_prefix(&prefix) {
            return Some(value.to_owned());
        }
    }
    None
}

/// `--manifest-path`, for commands Oxide does not otherwise parse.
pub fn manifest_path(args: &[OsString]) -> Option<PathBuf> {
    flag_value(args, "--manifest-path").map(PathBuf::from)
}

/// `-p`/`--package` without any `@version` suffix, before any `--`.
pub fn package_selection(args: &[OsString]) -> Option<String> {
    let compact = args
        .iter()
        .take_while(|argument| *argument != "--")
        .filter_map(|argument| argument.to_str())
        .find_map(|text| text.strip_prefix("-p").filter(|rest| !rest.is_empty()));
    let spec = flag_value(args, "--package")
        .or_else(|| flag_value(args, "-p"))
        .or_else(|| compact.map(str::to_owned))?;
    Some(spec.split('@').next().unwrap_or_default().to_owned())
}

/// `run`: the `--bin`/`--example` name the caller selected, if any.
pub fn run_selection(cargo_args: &[OsString]) -> Option<String> {
    flag_value(cargo_args, "--bin").or_else(|| flag_value(cargo_args, "--example"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn classes_follow_the_workflow_table() {
        for command in ["check", "test", "clippy", "build", "run"] {
            assert_eq!(classify(command), Class::Modelled, "{command}");
        }
        for command in [
            "doc", "bench", "rustc", "rustdoc", "fix", "nextest", "llvm-cov",
        ] {
            assert_eq!(classify(command), Class::Building, "{command}");
        }
        for command in ["fmt", "tree", "metadata", "version", "fetch"] {
            assert_eq!(classify(command), Class::Plain, "{command}");
        }
        assert_eq!(classify("clean"), Class::Clean);
        assert_eq!(classify("status"), Class::Status);
        assert_eq!(classify("init"), Class::Init);
        assert_eq!(classify("new"), Class::New);
    }

    #[test]
    fn modelled_commands_default_to_quick_and_translate_profiles() {
        let quick = parse_leased("check", &args(&["-p", "demo"])).unwrap();
        assert_eq!(quick.profile, Profile::Quick);
        assert_eq!(
            quick.cargo_args,
            args(&["check", "-p", "demo", "--profile", "oxide-quick"])
        );
        let debug =
            parse_leased("test", &args(&["--profile", "debug", "--", "--nocapture"])).unwrap();
        assert_eq!(
            debug.cargo_args,
            args(&["test", "--profile", "oxide-debug", "--", "--nocapture"])
        );
        let release = parse_leased("build", &args(&["--release"])).unwrap();
        assert_eq!(release.profile, Profile::Release);
        assert_eq!(release.cargo_args, args(&["build", "--release"]));
        assert!(parse_leased("build", &args(&["--release", "--profile=quick"])).is_err());
        assert!(parse_leased("build", &args(&["--profile", "dev"])).is_err());
        assert!(parse_leased("build", &args(&["--profile=quick", "--profile=debug"])).is_err());
    }

    #[test]
    fn run_builds_then_keeps_program_arguments() {
        let run = parse_leased(
            "run",
            &args(&[
                "--bin",
                "demo",
                "--message-format",
                "json",
                "--",
                "--flag",
                "x",
            ]),
        )
        .unwrap();
        assert_eq!(
            run.cargo_args,
            args(&[
                "build",
                "--bin",
                "demo",
                "--profile",
                "oxide-quick",
                "--message-format=json-render-diagnostics"
            ])
        );
        assert_eq!(run.program_args, args(&["--flag", "x"]));
        assert!(run.json_messages);
        assert_eq!(run_selection(&run.cargo_args).as_deref(), Some("demo"));
    }

    #[test]
    fn external_subcommands_are_untouched_and_builtins_translate_only_when_asked() {
        let external =
            parse_leased("nextest", &args(&["run", "--cargo-profile", "oxide-quick"])).unwrap();
        assert_eq!(
            external.cargo_args,
            args(&["nextest", "run", "--cargo-profile", "oxide-quick"])
        );
        let doc = parse_leased("doc", &args(&["--no-deps"])).unwrap();
        assert_eq!(doc.cargo_args, args(&["doc", "--no-deps"]));
        let bench = parse_leased("bench", &args(&["--profile", "debug"])).unwrap();
        assert_eq!(bench.profile, Profile::Debug);
        assert_eq!(
            bench.cargo_args,
            args(&["bench", "--profile", "oxide-debug"])
        );
    }

    #[test]
    fn storage_placement_overrides_refuse() {
        for refused in [
            &["--target-dir", "x"][..],
            &["--target-dir=x"],
            &["--config", "build.build-dir='x'"],
            &["--config=build.target-dir=\"x\""],
            &["--config", "build.target_dir = 'x'"],
            &["--config", "build = { build-dir = 'x' }"],
            &["--config", "extra.toml"],
        ] {
            assert!(
                refuse_storage_overrides(&args(refused)).is_err(),
                "{refused:?}"
            );
            assert!(
                parse_leased("nextest", &args(refused)).is_err(),
                "{refused:?}"
            );
        }
        assert!(refuse_storage_overrides(&args(&["--config", "net.offline=true"])).is_ok());
        assert!(refuse_storage_overrides(&args(&["--", "--target-dir", "x"])).is_ok());
    }

    #[test]
    fn package_selection_reads_every_form() {
        assert_eq!(
            package_selection(&args(&["-p", "app"])).as_deref(),
            Some("app")
        );
        assert_eq!(package_selection(&args(&["-papp"])).as_deref(), Some("app"));
        assert_eq!(
            package_selection(&args(&["--package=app@0.1.0"])).as_deref(),
            Some("app")
        );
        assert_eq!(package_selection(&args(&["--", "-p", "app"])), None);
    }

    #[test]
    fn manifest_path_and_target_are_observed() {
        let parsed = parse_leased(
            "build",
            &args(&[
                "--manifest-path=/w/Cargo.toml",
                "--target",
                "x86_64-apple-darwin",
            ]),
        )
        .unwrap();
        assert_eq!(parsed.manifest_path, Some(PathBuf::from("/w/Cargo.toml")));
        assert_eq!(parsed.target.as_deref(), Some("x86_64-apple-darwin"));
    }
}
