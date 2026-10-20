//! Inherited rustc wrapper policy for the Cargo calls Oxide owns (oxide#099).
//!
//! Leased builds bypass any inherited `build.rustc-wrapper` (today the
//! machine-wide sccache) by setting `RUSTC_WRAPPER` to the empty string.
//! Cargo 1.97.1 (`c980f486`) reads that variable before any config layer
//! (`GlobalContext::maybe_get_tool`, `util/context/mod.rs` 1805-1825), so it
//! beats config files, `CARGO_BUILD_RUSTC_WRAPPER` and `--config`; an empty
//! wrapper is then skipped (`ProcessBuilder::wrapped`,
//! `cargo-util/src/process_builder.rs` 549-557). Child Cargo calls, such as
//! an external subcommand's `$CARGO`, inherit the variable. The workspace
//! wrapper (`RUSTC_WORKSPACE_WRAPPER`, Clippy's driver) is a separate lookup
//! and is never touched. Only `OXIDE_ALLOW_RUSTC_WRAPPER=1` keeps the
//! inherited wrapper, which stays unqualified.

use crate::cargo_home;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The opt-in variable; only the exact value `1` enables it.
pub const ALLOW: &str = "OXIDE_ALLOW_RUSTC_WRAPPER";
/// Longest wrapper name or source shown in one line.
const MAX_SHOWN: usize = 160;

/// What Oxide does with an inherited rustc wrapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Policy {
    /// The default: Oxide's Cargo calls run without the wrapper.
    Bypass,
    /// `OXIDE_ALLOW_RUSTC_WRAPPER` is set to something other than `1`: the
    /// wrapper is still bypassed.
    BypassIgnoringOptIn,
    /// `OXIDE_ALLOW_RUSTC_WRAPPER=1`: Cargo resolves the wrapper as usual.
    Keep,
}

impl Policy {
    pub fn from_env() -> Self {
        Self::from_value(std::env::var_os(ALLOW).as_deref())
    }

    fn from_value(value: Option<&OsStr>) -> Self {
        match value {
            None => Self::Bypass,
            Some(value) if value == "1" => Self::Keep,
            Some(_) => Self::BypassIgnoringOptIn,
        }
    }

    fn bypasses(self) -> bool {
        self != Self::Keep
    }
}

/// Apply the policy to one Cargo call Oxide owns. Oxide's own environment
/// is never changed; plain passthrough never reaches here.
pub fn apply(command: &mut Command) {
    if Policy::from_env().bypasses() {
        command.env("RUSTC_WRAPPER", "");
    }
}

/// The rustc wrapper Cargo would use without Oxide, as far as Oxide can read
/// it. Diagnostic only: the bypass does not depend on it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Inherited {
    None,
    Named {
        program: String,
        source: String,
    },
    /// A layer Oxide cannot read may set one.
    Unknown(String),
}

/// Read the inherited wrapper in Cargo's order: `RUSTC_WRAPPER`, then the
/// command's `--config` values (the last wins), `CARGO_BUILD_RUSTC_WRAPPER`,
/// and config files from the current directory up, then the Cargo home.
/// `config_args` are Cargo's own arguments; pass none for an external
/// subcommand, whose arguments Cargo does not read.
pub fn inherited(config_args: &[OsString]) -> Inherited {
    let files = match (std::env::current_dir(), cargo_home::resolve()) {
        (Ok(cwd), Ok(home)) => cargo_home::config_files(&cwd, &home),
        (Err(error), _) => {
            return Inherited::Unknown(format!("cannot read the current directory: {error}"))
        }
        (_, Err(error)) => return Inherited::Unknown(error),
    };
    inherited_from(
        std::env::var_os("RUSTC_WRAPPER").as_deref(),
        &config_values(config_args),
        std::env::var_os("CARGO_BUILD_RUSTC_WRAPPER").as_deref(),
        &files,
        |path| cargo_home::read_config(path),
    )
}

fn inherited_from(
    environment: Option<&OsStr>,
    configs: &[String],
    build_environment: Option<&OsStr>,
    files: &[PathBuf],
    read: impl Fn(&Path) -> Result<serde_json::Value, String>,
) -> Inherited {
    if let Some(value) = environment {
        return named(value, "RUSTC_WRAPPER");
    }
    for config in configs.iter().rev() {
        match basic_toml::from_str::<serde_json::Value>(config) {
            Ok(parsed) => match from_document(&parsed) {
                Found::Value(value) => return program(value, "--config"),
                Found::Unreadable => {
                    return Inherited::Unknown("a --config value is not a string".to_owned())
                }
                Found::Absent | Found::Included => {}
            },
            Err(_) if mentions_wrapper(config) => {
                return Inherited::Unknown("a --config value Oxide cannot parse".to_owned())
            }
            Err(_) => {}
        }
    }
    if let Some(value) = build_environment {
        return named(value, "CARGO_BUILD_RUSTC_WRAPPER");
    }
    for file in files {
        let source = shown(&file.display().to_string());
        let parsed = match read(file) {
            Ok(parsed) => parsed,
            Err(_) => return Inherited::Unknown(format!("{source} cannot be read")),
        };
        match from_document(&parsed) {
            Found::Value(value) => return program(value, &source),
            Found::Unreadable => {
                return Inherited::Unknown(format!("{source} sets it to a non-string"))
            }
            Found::Included => {
                return Inherited::Unknown(format!("{source} includes another config file"))
            }
            Found::Absent => {}
        }
    }
    Inherited::None
}

enum Found<'a> {
    Absent,
    Value(&'a str),
    Unreadable,
    /// `include` without the key: the included file may set it.
    Included,
}

fn from_document(document: &serde_json::Value) -> Found<'_> {
    let build = document.get("build");
    let value = build
        .and_then(|build| build.get("rustc-wrapper"))
        .or_else(|| build.and_then(|build| build.get("rustc_wrapper")));
    match value {
        Some(value) => value.as_str().map_or(Found::Unreadable, Found::Value),
        None if document.get("include").is_some() => Found::Included,
        None => Found::Absent,
    }
}

fn mentions_wrapper(config: &str) -> bool {
    config.contains("rustc-wrapper") || config.contains("rustc_wrapper")
}

fn named(value: &OsStr, source: &str) -> Inherited {
    match value.to_str() {
        Some(value) => program(value, source),
        None => Inherited::Unknown(format!("{source} is not valid UTF-8")),
    }
}

/// An empty value means no wrapper, as Cargo treats it.
fn program(value: &str, source: &str) -> Inherited {
    if value.is_empty() {
        Inherited::None
    } else {
        Inherited::Named {
            program: shown(value),
            source: source.to_owned(),
        }
    }
}

/// `--config` values before any `--`, in order.
fn config_values(args: &[OsString]) -> Vec<String> {
    let mut values = Vec::new();
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        let Some(text) = argument.to_str() else {
            continue;
        };
        if text == "--" {
            break;
        }
        if text == "--config" {
            if let Some(value) = iter.next().and_then(|value| value.to_str()) {
                values.push(value.to_owned());
            }
        } else if let Some(value) = text.strip_prefix("--config=") {
            values.push(value.to_owned());
        }
    }
    values
}

/// One line, bounded: control characters escaped, long text cut.
fn shown(text: &str) -> String {
    let escaped: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                character.escape_default().collect()
            } else {
                character.to_string()
            }
        })
        .collect();
    if escaped.chars().count() <= MAX_SHOWN {
        escaped
    } else {
        let cut: String = escaped.chars().take(MAX_SHOWN).collect();
        format!("{cut}…")
    }
}

/// The summary-line fragment for a leased command, or nothing when there is
/// nothing to report.
pub fn summary(policy: Policy, inherited: &Inherited) -> Option<String> {
    let ignored = format!("{ALLOW} is not 1, so ignored");
    match (policy, inherited) {
        (Policy::Bypass, Inherited::None) => None,
        (Policy::BypassIgnoringOptIn, Inherited::None) => Some(ignored),
        (Policy::Bypass, Inherited::Named { program, .. }) => {
            Some(format!("rustc wrapper bypassed: {program}"))
        }
        (Policy::BypassIgnoringOptIn, Inherited::Named { program, .. }) => {
            Some(format!("rustc wrapper bypassed: {program} ({ignored})"))
        }
        (Policy::Bypass, Inherited::Unknown(why)) => {
            Some(format!("any rustc wrapper bypassed (inherited one unknown: {why})"))
        }
        (Policy::BypassIgnoringOptIn, Inherited::Unknown(why)) => Some(format!(
            "any rustc wrapper bypassed (inherited one unknown: {why}; {ignored})"
        )),
        (Policy::Keep, Inherited::None) => {
            Some(format!("{ALLOW}=1 but no inherited rustc wrapper found"))
        }
        (Policy::Keep, Inherited::Named { program, .. }) => Some(format!(
            "rustc wrapper active: {program}, unqualified for Oxide's interruption and toolchain identity ({ALLOW}=1)"
        )),
        (Policy::Keep, Inherited::Unknown(why)) => Some(format!(
            "any inherited rustc wrapper active (unknown: {why}), unqualified for Oxide's interruption and toolchain identity ({ALLOW}=1)"
        )),
    }
}

/// The `status` line: the inherited wrapper and its effective state.
pub fn status_line(policy: Policy, inherited: &Inherited) -> String {
    let inherited_text = match inherited {
        Inherited::None => "none".to_owned(),
        Inherited::Named { program, source } => format!("{program} (from {source})"),
        Inherited::Unknown(why) => format!("unknown ({why})"),
    };
    let effect = match policy {
        Policy::Bypass => "bypassed for oxide builds".to_owned(),
        Policy::BypassIgnoringOptIn => {
            format!("bypassed for oxide builds; {ALLOW} is not 1, so ignored")
        }
        Policy::Keep => format!(
            "kept for oxide builds by {ALLOW}=1, unqualified for Oxide's interruption and toolchain identity"
        ),
    };
    format!("rustc wrapper: inherited {inherited_text}; {effect}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> Option<&OsStr> {
        Some(OsStr::new(value))
    }

    fn sccache(source: &str) -> Inherited {
        Inherited::Named {
            program: "/opt/homebrew/bin/sccache".to_owned(),
            source: source.to_owned(),
        }
    }

    fn reader(
        files: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&Path) -> Result<serde_json::Value, String> {
        move |path| {
            let (_, text) = files
                .iter()
                .find(|(name, _)| Path::new(name) == path)
                .ok_or("missing")?;
            basic_toml::from_str(text).map_err(|_| "unparseable".to_owned())
        }
    }

    #[test]
    fn only_exactly_one_keeps_the_wrapper() {
        assert_eq!(Policy::from_value(None), Policy::Bypass);
        assert_eq!(Policy::from_value(os("1")), Policy::Keep);
        for other in ["", "0", "true", "yes", " 1", "1 ", "01"] {
            assert_eq!(
                Policy::from_value(os(other)),
                Policy::BypassIgnoringOptIn,
                "{other:?}"
            );
        }
    }

    #[test]
    fn inherited_wrapper_follows_cargo_precedence() {
        let files: &'static [(&'static str, &'static str)] = &[
            ("/w/.cargo/config.toml", "[build]\njobs = 2\n"),
            (
                "/home/config.toml",
                "[build]\nrustc-wrapper = \"/opt/homebrew/bin/sccache\"\n",
            ),
        ];
        let paths: Vec<PathBuf> = files.iter().map(|(name, _)| PathBuf::from(name)).collect();
        let found = |environment, configs: &[&str], build| {
            let configs: Vec<String> = configs.iter().map(|value| (*value).to_owned()).collect();
            inherited_from(environment, &configs, build, &paths, reader(files))
        };
        assert_eq!(found(None, &[], None), sccache("/home/config.toml"));
        assert_eq!(
            found(
                os("/opt/homebrew/bin/sccache"),
                &["build.rustc-wrapper='x'"],
                None
            ),
            sccache("RUSTC_WRAPPER")
        );
        assert_eq!(found(os(""), &[], None), Inherited::None);
        assert_eq!(
            found(
                None,
                &[
                    "build.rustc-wrapper='first'",
                    "build.rustc-wrapper='/opt/homebrew/bin/sccache'"
                ],
                os("other")
            ),
            sccache("--config")
        );
        assert_eq!(
            found(None, &["net.offline=true"], os("/opt/homebrew/bin/sccache")),
            sccache("CARGO_BUILD_RUSTC_WRAPPER")
        );
        assert_eq!(found(None, &[], os("")), Inherited::None);
    }

    #[test]
    fn the_nearest_config_file_wins_and_unreadable_layers_are_unknown() {
        let files: &'static [(&'static str, &'static str)] = &[
            ("/w/.cargo/config.toml", "build.rustc-wrapper = \"near\"\n"),
            ("/home/config.toml", "[build]\nrustc-wrapper = \"far\"\n"),
        ];
        let paths: Vec<PathBuf> = files.iter().map(|(name, _)| PathBuf::from(name)).collect();
        assert_eq!(
            inherited_from(None, &[], None, &paths, reader(files)),
            Inherited::Named {
                program: "near".to_owned(),
                source: "/w/.cargo/config.toml".to_owned()
            }
        );
        let cases: [(&'static [(&'static str, &'static str)], &str); 3] = [
            (&[("/a", "not = [toml")], "/a cannot be read"),
            (
                &[("/a", "include = \"other.toml\"\n")],
                "/a includes another config file",
            ),
            (
                &[("/a", "build.rustc-wrapper = 3\n")],
                "/a sets it to a non-string",
            ),
        ];
        for (files, why) in cases {
            assert_eq!(
                inherited_from(None, &[], None, &[PathBuf::from("/a")], reader(files)),
                Inherited::Unknown(why.to_owned())
            );
        }
        assert_eq!(
            inherited_from(
                None,
                &["build.rustc-wrapper".to_owned()],
                None,
                &[],
                reader(&[])
            ),
            Inherited::Unknown("a --config value Oxide cannot parse".to_owned())
        );
        assert_eq!(
            inherited_from(None, &[], None, &[], reader(&[])),
            Inherited::None
        );
    }

    #[test]
    fn config_values_stop_at_the_separator() {
        let args: Vec<OsString> = [
            "check",
            "--config",
            "a=1",
            "--config=b=2",
            "--",
            "--config",
            "c=3",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(config_values(&args), ["a=1", "b=2"]);
    }

    #[test]
    fn shown_names_are_one_bounded_line() {
        assert_eq!(
            shown("/opt/homebrew/bin/sccache"),
            "/opt/homebrew/bin/sccache"
        );
        assert_eq!(shown("a\nb"), "a\\nb");
        let long = shown(&"x".repeat(500));
        assert_eq!(long.chars().count(), MAX_SHOWN + 1);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn summary_and_status_name_the_policy() {
        let named = sccache("/home/config.toml");
        let unknown = Inherited::Unknown("/a cannot be read".to_owned());
        assert_eq!(summary(Policy::Bypass, &Inherited::None), None);
        assert_eq!(
            summary(Policy::Bypass, &named).as_deref(),
            Some("rustc wrapper bypassed: /opt/homebrew/bin/sccache")
        );
        assert_eq!(
            summary(Policy::Bypass, &unknown).as_deref(),
            Some("any rustc wrapper bypassed (inherited one unknown: /a cannot be read)")
        );
        let kept = summary(Policy::Keep, &named).unwrap_or_default();
        assert!(kept.starts_with("rustc wrapper active: /opt/homebrew/bin/sccache, unqualified"));
        let ignored = summary(Policy::BypassIgnoringOptIn, &named).unwrap_or_default();
        assert!(ignored.starts_with("rustc wrapper bypassed:"), "{ignored}");
        assert!(ignored.contains("is not 1, so ignored"), "{ignored}");
        assert_eq!(
            status_line(Policy::Bypass, &named),
            "rustc wrapper: inherited /opt/homebrew/bin/sccache (from /home/config.toml); bypassed for oxide builds"
        );
        assert_eq!(
            status_line(Policy::Bypass, &Inherited::None),
            "rustc wrapper: inherited none; bypassed for oxide builds"
        );
        assert!(status_line(Policy::Keep, &unknown)
            .starts_with("rustc wrapper: inherited unknown (/a cannot be read); kept"));
    }
}
