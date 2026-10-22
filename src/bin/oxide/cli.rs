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
    /// `pool`: Oxide-owned pool registration; not a Cargo built-in.
    Pool,
    /// `outputs`: Oxide-owned final-output placement; not a Cargo built-in.
    Outputs,
    /// `exec`: a tool that calls Cargo itself, leased; not a Cargo built-in.
    Exec,
}

pub fn classify(command: &str) -> Class {
    match command {
        "check" | "test" | "clippy" | "build" | "run" => Class::Modelled,
        "clean" => Class::Clean,
        "status" => Class::Status,
        "pool" => Class::Pool,
        "outputs" => Class::Outputs,
        "exec" => Class::Exec,
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

/// Whether Cargo itself reads this leased command's `--config` arguments.
/// An external subcommand receives them as its own.
pub fn reads_config_args(command: &str) -> bool {
    classify(command) == Class::Modelled || is_building_builtin(command)
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

    /// The recipe this profile selects, in the slot compatibility key.
    /// `quick` and `debug` share Cargo's `dev` name, so the key names the
    /// recipe. Version 2 tunes Cargo's own `dev` profile through
    /// [`Profile::dev_recipe`]; version 1 keys (the bare labels) built the
    /// custom `oxide-quick` / `oxide-debug` profiles. Release is unchanged.
    pub fn recipe_key(self) -> &'static str {
        match self {
            Self::Quick => "quick:dev-recipe-2",
            Self::Debug => "debug:dev-recipe-2",
            Self::Release => "release",
        }
    }

    /// Cargo's `dev` settings this profile sets (`Some`) or clears (`None`)
    /// in every leased child's environment. Child Cargo calls, including an
    /// external subcommand's `$CARGO`, inherit them; `test` inherits `dev`.
    /// Clearing keeps an outer `debug` command's build override out of a
    /// nested `quick` one. Release leaves `dev` as the caller has it.
    pub fn dev_recipe(self) -> &'static [(&'static str, Option<&'static str>)] {
        match self {
            Self::Quick => &[
                ("CARGO_PROFILE_DEV_DEBUG", Some("line-tables-only")),
                ("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG", None),
            ],
            Self::Debug => &[
                ("CARGO_PROFILE_DEV_DEBUG", Some("full")),
                ("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG", Some("full")),
            ],
            Self::Release => &[],
        }
    }
}

/// Deprecated custom profiles, still defined for every leased command so an
/// external tool's existing `--cargo-profile oxide-quick` or `oxide-debug`
/// keeps building. Their outputs stay in `target/oxide-quick` and
/// `target/oxide-debug`; only Oxide's own profiles moved to `target/debug`.
pub const LEGACY_PROFILE_ENVIRONMENT: [(&str, &str); 5] = [
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
    /// `exec` only: the tool and its arguments, spawned as given.
    pub tool: Vec<OsString>,
}

pub const EXEC_USAGE: &str = "\
USAGE:
    oxide exec [--manifest-path <Cargo.toml>] [--profile quick|debug|release] -- <command> [args]

Runs one tool that calls Cargo itself (a Tauri CLI, an npm or bun script) for
one Cargo workspace, with Oxide's storage, profile recipe and rustc-wrapper
policy in its environment. The workspace comes from the current directory or
--manifest-path; the tool keeps this working directory and its arguments.
--profile selects the recipe and slot only: the tool still picks its own
dev or release build. Final outputs stay in <workspace>/target. The tool runs
as leader of its own process group; the slot stays held until every process
in that group has exited. Its exit code is returned (128 + signal when a
signal ended it). A terminal standard input is not passed: the tool reads
end-of-file. Cooperative only: a tool that sets its own target or build
directory, changes workspace or toolchain, or leaves its process group with
setsid escapes Oxide's storage and closure.
";

/// Parse `oxide exec`. Oxide's options come before the mandatory `--`; the
/// command after it is kept exactly. `Ok(None)` asks for help.
pub fn parse_exec(args: &[OsString]) -> Result<Option<Invocation>, String> {
    let mut profile = None;
    let mut manifest_path = None;
    let mut iter = args.iter();
    let mut tool = None;
    while let Some(argument) = iter.next() {
        let text = argument
            .to_str()
            .ok_or("exec options must be valid UTF-8")?;
        if text == "--" {
            tool = Some(iter.by_ref().cloned().collect::<Vec<_>>());
            break;
        }
        if text == "-h" || text == "--help" {
            return Ok(None);
        }
        let mut value = |flag: &str| -> Result<Option<String>, String> {
            if text == flag {
                return iter
                    .next()
                    .and_then(|value| value.to_str())
                    .map(|value| Some(value.to_owned()))
                    .ok_or(format!("{flag} requires a value"));
            }
            Ok(text.strip_prefix(&format!("{flag}=")).map(str::to_owned))
        };
        if let Some(path) = value("--manifest-path")? {
            if path.is_empty() {
                return Err("--manifest-path requires a value".to_owned());
            }
            if manifest_path.replace(PathBuf::from(path)).is_some() {
                return Err("--manifest-path may be given only once".to_owned());
            }
            continue;
        }
        if let Some(name) = value("--profile")? {
            if profile.replace(Profile::parse(&name)?).is_some() {
                return Err("--profile may be given only once".to_owned());
            }
            continue;
        }
        if text == "--release" || text == "-r" {
            return Err("exec takes --profile release, not --release".to_owned());
        }
        refuse_storage_overrides(std::slice::from_ref(argument))?;
        return Err(format!(
            "exec does not take {text}; put the command after `--`: oxide exec -- <command>"
        ));
    }
    let tool = tool.ok_or("exec needs `--` before the command: oxide exec -- <command>")?;
    if tool.first().is_none_or(|program| program.is_empty()) {
        return Err("exec needs a command after `--`".to_owned());
    }
    Ok(Some(Invocation {
        command: "exec".to_owned(),
        profile: profile.unwrap_or(Profile::Quick),
        cargo_args: Vec::new(),
        program_args: Vec::new(),
        json_messages: false,
        manifest_path,
        target: None,
        tool,
    }))
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
            tool: Vec::new(),
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
    let profile = if release {
        Profile::Release
    } else {
        selected.unwrap_or(Profile::Quick)
    };
    // Oxide consumes its own `--profile` and never forwards `quick` or
    // `debug` as a Cargo profile name, even where a newer Cargo accepts
    // `--profile debug`. Their recipe is `dev` tuned by the environment, so
    // commands keep Cargo's default profile (`dev`, or `test` inheriting it).
    // `bench` defaults to a release-based profile, so an explicit quick or
    // debug bench names `dev`.
    let flags = match profile {
        Profile::Release => vec![OsString::from("--release")],
        Profile::Quick | Profile::Debug if command == "bench" && selected.is_some() => {
            vec![OsString::from("--profile"), OsString::from("dev")]
        }
        _ => Vec::new(),
    };
    let position = cargo_args
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(cargo_args.len());
    cargo_args.splice(position..position, flags);
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
        tool: Vec::new(),
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

/// What the guarded command asks of Cargo's dependency resolution.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Resolution {
    /// Its `--locked`, `--frozen`, `--offline`, `--config`, `-Z` and
    /// `--lockfile-path` arguments before any `--`, unchanged and in order.
    /// Cargo merges them with inherited configuration and environment, so
    /// precedence stays Cargo's.
    pub args: Vec<OsString>,
    /// `-h` or `--help`: Cargo prints help and resolves nothing.
    pub help: bool,
    /// For a built-in Cargo command that resolves the workspace, and only
    /// when it passes no `-Z` of its own: its Cargo arguments with
    /// `-Z oxide-parse-check` before any `--`. Cargo parses every argument,
    /// then refuses the unknown `-Z` before it loads the workspace, so the
    /// call has no effects and tells whether Cargo accepts the command.
    pub parse_check: Option<Vec<OsString>>,
}

/// Built-in Cargo commands whose run resolves the workspace and writes a
/// missing lock file (`run` builds through `build`).
const RESOLVING: [&str; 7] = ["build", "check", "test", "bench", "doc", "rustc", "rustdoc"];
pub const PARSE_CHECK: &str = "oxide-parse-check";

impl Resolution {
    fn has(&self, flag: &str) -> bool {
        self.args.iter().any(|argument| argument == flag)
    }

    /// `--locked` or `--frozen`: Cargo must not write the lock file.
    pub fn locked(&self) -> bool {
        self.has("--locked") || self.has("--frozen")
    }

    /// `--offline` or `--frozen`: offline whatever config or environment say.
    pub fn offline(&self) -> bool {
        self.has("--offline") || self.has("--frozen")
    }

    /// `--lockfile-path`: the lock file is not the workspace's `Cargo.lock`.
    pub fn moves_lockfile(&self) -> bool {
        self.args.iter().any(|argument| {
            argument == "--lockfile-path"
                || argument
                    .to_str()
                    .is_some_and(|text| text.starts_with("--lockfile-path="))
        })
    }
}

/// The resolution request in a leased command's Cargo arguments.
pub fn resolution(cargo_args: &[OsString]) -> Resolution {
    let mut request = Resolution::default();
    let mut iter = cargo_args.iter().skip(1);
    while let Some(argument) = iter.next() {
        let text = argument.to_str().unwrap_or_default();
        match text {
            "--" => break,
            "-h" | "--help" => request.help = true,
            "--locked" | "--frozen" | "--offline" => request.args.push(argument.clone()),
            "--config" | "-Z" | "--lockfile-path" => {
                request.args.push(argument.clone());
                request.args.extend(iter.next().cloned());
            }
            _ if text.starts_with("--config=")
                || text.starts_with("--lockfile-path=")
                || text.starts_with("-Z") =>
            {
                request.args.push(argument.clone());
            }
            _ => {}
        }
    }
    let resolving = cargo_args
        .first()
        .and_then(|command| command.to_str())
        .is_some_and(|command| RESOLVING.contains(&command));
    let own_unstable = request
        .args
        .iter()
        .any(|argument| argument.to_str().is_some_and(|text| text.starts_with("-Z")));
    if resolving && !own_unstable {
        let position = cargo_args
            .iter()
            .position(|argument| argument == "--")
            .unwrap_or(cargo_args.len());
        let mut check = cargo_args.to_vec();
        check.splice(
            position..position,
            [OsString::from("-Z"), OsString::from(PARSE_CHECK)],
        );
        request.parse_check = Some(check);
    }
    request
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
        assert_eq!(classify("pool"), Class::Pool);
        assert_eq!(classify("exec"), Class::Exec);
        assert_eq!(classify("outputs"), Class::Outputs);
        assert_eq!(classify("init"), Class::Init);
        assert_eq!(classify("new"), Class::New);
    }

    #[test]
    fn modelled_commands_consume_oxide_profiles_and_keep_cargo_defaults() {
        let quick = parse_leased("check", &args(&["-p", "demo"])).unwrap();
        assert_eq!(quick.profile, Profile::Quick);
        assert_eq!(quick.cargo_args, args(&["check", "-p", "demo"]));
        // Both spellings of `--profile debug` are Oxide's own: never
        // forwarded, even though Cargo 1.99 accepts `--profile debug`.
        for spelling in [&["--profile", "debug"][..], &["--profile=debug"]] {
            let mut given = spelling.to_vec();
            given.extend(["--", "--nocapture"]);
            let debug = parse_leased("test", &args(&given)).unwrap();
            assert_eq!(debug.profile, Profile::Debug);
            assert_eq!(debug.cargo_args, args(&["test", "--", "--nocapture"]));
        }
        let explicit = parse_leased("build", &args(&["--profile", "quick"])).unwrap();
        assert_eq!(explicit.profile, Profile::Quick);
        assert_eq!(explicit.cargo_args, args(&["build"]));
        let release = parse_leased("build", &args(&["--release"])).unwrap();
        assert_eq!(release.profile, Profile::Release);
        assert_eq!(release.cargo_args, args(&["build", "--release"]));
        let named = parse_leased(
            "clippy",
            &args(&["--profile=release", "--", "-D", "warnings"]),
        );
        assert_eq!(
            named.unwrap().cargo_args,
            args(&["clippy", "--release", "--", "-D", "warnings"])
        );
        assert!(parse_leased("build", &args(&["--release", "--profile=quick"])).is_err());
        assert!(parse_leased("build", &args(&["--profile", "dev"])).is_err());
        assert!(parse_leased("build", &args(&["--profile", "oxide-quick"])).is_err());
        assert!(parse_leased("build", &args(&["--profile=quick", "--profile=debug"])).is_err());
    }

    #[test]
    fn recipes_tune_dev_and_keys_separate_them() {
        assert_eq!(
            Profile::Quick.dev_recipe(),
            &[
                ("CARGO_PROFILE_DEV_DEBUG", Some("line-tables-only")),
                ("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG", None),
            ]
        );
        assert_eq!(
            Profile::Debug.dev_recipe(),
            &[
                ("CARGO_PROFILE_DEV_DEBUG", Some("full")),
                ("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG", Some("full")),
            ]
        );
        assert!(Profile::Release.dev_recipe().is_empty());
        let keys = [Profile::Quick, Profile::Debug, Profile::Release].map(Profile::recipe_key);
        assert_eq!(
            keys,
            ["quick:dev-recipe-2", "debug:dev-recipe-2", "release"]
        );
        // Version-1 keys were the bare labels; a changed recipe never meets
        // one. Release kept its recipe and its key.
        assert_ne!(Profile::Quick.recipe_key(), Profile::Quick.label());
        assert_ne!(Profile::Debug.recipe_key(), Profile::Debug.label());
        assert_eq!(Profile::Release.recipe_key(), Profile::Release.label());
        // The deprecated custom profiles stay defined for existing adopters.
        let legacy: Vec<&str> = LEGACY_PROFILE_ENVIRONMENT
            .iter()
            .map(|(name, _)| *name)
            .collect();
        assert!(legacy.contains(&"CARGO_PROFILE_OXIDE_QUICK_INHERITS"));
        assert!(legacy.contains(&"CARGO_PROFILE_OXIDE_DEBUG_BUILD_OVERRIDE_DEBUG"));
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
        let tool = parse_leased("nextest", &args(&["run", "--cargo-profile", "debug"])).unwrap();
        assert_eq!(tool.profile, Profile::Quick);
        assert_eq!(
            tool.cargo_args,
            args(&["nextest", "run", "--cargo-profile", "debug"])
        );
        let doc = parse_leased("doc", &args(&["--no-deps"])).unwrap();
        assert_eq!(doc.cargo_args, args(&["doc", "--no-deps"]));
        let doc = parse_leased("doc", &args(&["--profile", "debug", "--no-deps"])).unwrap();
        assert_eq!(doc.profile, Profile::Debug);
        assert_eq!(doc.cargo_args, args(&["doc", "--no-deps"]));
        // `bench` keeps its own release-based default; an explicit quick or
        // debug bench names `dev`, whose recipe the environment sets.
        let bench = parse_leased("bench", &args(&[])).unwrap();
        assert_eq!(bench.profile, Profile::Quick);
        assert_eq!(bench.cargo_args, args(&["bench"]));
        let bench = parse_leased("bench", &args(&["--profile", "debug"])).unwrap();
        assert_eq!(bench.profile, Profile::Debug);
        assert_eq!(bench.cargo_args, args(&["bench", "--profile", "dev"]));
        let bench = parse_leased("bench", &args(&["--release", "--", "--exact"])).unwrap();
        assert_eq!(bench.profile, Profile::Release);
        assert_eq!(
            bench.cargo_args,
            args(&["bench", "--release", "--", "--exact"])
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
    fn exec_parses_oxide_options_and_keeps_the_command_exactly() {
        use std::os::unix::ffi::OsStringExt;
        let odd = OsString::from_vec(vec![b'a', 0xff, b'b']);
        let mut given = args(&[
            "--manifest-path",
            "app/Cargo.toml",
            "--profile=debug",
            "--",
            "npm",
            "run",
            "--profile",
            "release",
            "--target-dir=x",
            "--",
        ]);
        given.push(odd.clone());
        let parsed = parse_exec(&given).unwrap().unwrap();
        assert_eq!(parsed.command, "exec");
        assert_eq!(parsed.profile, Profile::Debug);
        assert_eq!(parsed.manifest_path, Some(PathBuf::from("app/Cargo.toml")));
        let mut tool = args(&["npm", "run", "--profile", "release", "--target-dir=x", "--"]);
        tool.push(odd);
        assert_eq!(parsed.tool, tool);
        assert!(parsed.cargo_args.is_empty() && parsed.target.is_none());
        let plain = parse_exec(&args(&["--", "tauri", "build"]))
            .unwrap()
            .unwrap();
        assert_eq!(plain.profile, Profile::Quick);
        assert_eq!(plain.manifest_path, None);
        let release = parse_exec(&args(&["--profile", "release", "--", "x"]))
            .unwrap()
            .unwrap();
        assert_eq!(release.profile, Profile::Release);
        assert_eq!(parse_exec(&args(&["--help"])), Ok(None));
        assert_eq!(parse_exec(&args(&["-h", "--", "x"])), Ok(None));
        // After `--`, help belongs to the tool.
        assert!(parse_exec(&args(&["--", "x", "--help"])).unwrap().is_some());
    }

    #[test]
    fn exec_refuses_malformed_input_before_any_effect() {
        for refused in [
            &[][..],
            &["tauri", "build"],
            &["--"],
            &["--", ""],
            &["--profile"],
            &["--profile", "dev", "--", "x"],
            &["--profile=quick", "--profile=debug", "--", "x"],
            &["--release", "--", "x"],
            &["--manifest-path"],
            &["--manifest-path=", "--", "x"],
            &[
                "--manifest-path",
                "a/Cargo.toml",
                "--manifest-path=b/Cargo.toml",
                "--",
                "x",
            ],
            &["--target-dir", "x", "--", "y"],
            &["--config", "build.build-dir='x'", "--", "y"],
            &["--offline", "--", "y"],
        ] {
            assert!(parse_exec(&args(refused)).is_err(), "{refused:?}");
        }
        let storage = parse_exec(&args(&["--target-dir=x", "--", "y"])).unwrap_err();
        assert!(storage.contains("Oxide places build output"), "{storage}");
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
    fn resolution_args_mirror_lock_network_and_config_flags() {
        let parsed = parse_leased(
            "test",
            &args(&[
                "--locked",
                "-p",
                "app",
                "--frozen",
                "--offline",
                "--config",
                "net.offline=false",
                "--config=net.offline=true",
                "-Z",
                "unstable-options",
                "-Zno-index-update",
                "--lockfile-path",
                "/w/other.lock",
                "--features=locked",
                "--",
                "--locked",
                "--offline",
            ]),
        )
        .unwrap();
        assert_eq!(
            resolution(&parsed.cargo_args).args,
            args(&[
                "--locked",
                "--frozen",
                "--offline",
                "--config",
                "net.offline=false",
                "--config=net.offline=true",
                "-Z",
                "unstable-options",
                "-Zno-index-update",
                "--lockfile-path",
                "/w/other.lock",
            ])
        );
        let request = resolution(&parsed.cargo_args);
        assert!(request.locked() && request.offline() && request.moves_lockfile());
        assert!(!request.help);
        assert_eq!(request.parse_check, None, "the command passes its own -Z");
        // No restriction requested: Cargo's own defaults.
        let plain = parse_leased("check", &args(&["-p", "app"])).unwrap();
        let plain = resolution(&plain.cargo_args);
        assert!(plain.args.is_empty() && !plain.help);
        assert_eq!(
            plain.parse_check,
            Some(args(&["check", "-p", "app", "-Z", PARSE_CHECK]))
        );
        let locked = resolution(
            &parse_leased("test", &args(&["--locked"]))
                .unwrap()
                .cargo_args,
        );
        assert!(locked.locked() && !locked.offline() && !locked.moves_lockfile());
        let offline = resolution(
            &parse_leased("test", &args(&["--offline"]))
                .unwrap()
                .cargo_args,
        );
        assert!(!offline.locked() && offline.offline());
        // Help resolves nothing; after `--` it is the test binary's.
        let help = parse_leased("test", &args(&["--locked", "--help"])).unwrap();
        assert!(resolution(&help.cargo_args).help);
        let short = parse_leased("build", &args(&["-h"])).unwrap();
        assert!(resolution(&short.cargo_args).help);
        let passed = parse_leased("test", &args(&["--", "--help"])).unwrap();
        assert!(!resolution(&passed.cargo_args).help);
        // The parse check goes before `--`, so it never reaches a test binary.
        assert_eq!(
            resolution(&passed.cargo_args).parse_check,
            Some(args(&["test", "-Z", PARSE_CHECK, "--", "--help"]))
        );
        // External subcommands are scanned after their name too, and have no
        // parse check: Cargo does not parse their arguments.
        let external = parse_leased("nextest", &args(&["run", "--frozen"])).unwrap();
        assert_eq!(resolution(&external.cargo_args).args, args(&["--frozen"]));
        assert_eq!(resolution(&external.cargo_args).parse_check, None);
        let clippy = parse_leased("clippy", &args(&[])).unwrap();
        assert_eq!(resolution(&clippy.cargo_args).parse_check, None);
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
