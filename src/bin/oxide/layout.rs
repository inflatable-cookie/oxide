//! Profile output members: which directories under `<checkout>/target` one
//! command writes.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Owned output
//! members at profile directories). `<checkout>/target` itself stays a real
//! directory; only positively identified profile members — `debug`,
//! `release`, `<triple>/debug` and `<triple>/release` — become owned links.
//! Cargo's top-level files (`CACHEDIR.TAG`, `.rustc_info.json`), `target/tmp`
//! and tool directories such as `target/doc` stay real.
//!
//! This module owns the pure part: member names, their bounded validation,
//! the profile mapping of one command and the complete declaration a caller
//! gives for an opaque tool (`oxide exec --output <member>`). It writes
//! nothing and reads no filesystem state.

use crate::cli::Profile;

/// Members one command may select, after deduplication and target expansion.
pub(crate) const MAX_MEMBERS: usize = 8;
/// Members one receipt may record.
pub(crate) const MAX_RECEIPT_MEMBERS: usize = 32;
/// Components of a member name: an optional triple, then the profile.
const MAX_COMPONENTS: usize = 2;

/// One profile output directory under `target/`: `debug`, `release`,
/// `<triple>/debug` or `<triple>/release`.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Member {
    name: String,
}

impl Member {
    /// Parse one member name. Only the four v1 forms are accepted: a plain
    /// profile directory, or one plain triple component in front of it. Every
    /// component matches `[A-Za-z0-9_.-]+` and is neither `.` nor `..`, so a
    /// member can never be an absolute path, an escape or a path with an
    /// empty, hidden-by-parent or traversing component.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let refused = || {
            format!(
                "{text:?} is not an output member; use debug, release, <triple>/debug or <triple>/release"
            )
        };
        let parts: Vec<&str> = text.split('/').collect();
        if parts.is_empty() || parts.len() > MAX_COMPONENTS || parts.iter().any(|part| !plain(part))
        {
            return Err(refused());
        }
        if !matches!(parts[parts.len() - 1], "debug" | "release") {
            return Err(refused());
        }
        Ok(Self {
            name: text.to_owned(),
        })
    }

    /// The host profile member a recipe writes: `debug` for `quick` and
    /// `debug`, `release` for release.
    pub(crate) fn host(profile: Profile) -> Self {
        Self {
            name: profile_member(profile).to_owned(),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The member's path under the checkout's real `target` directory.
    pub(crate) fn relative(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.name)
    }

    /// Whether this member names an explicit triple.
    pub(crate) fn triple(&self) -> Option<&str> {
        self.name.split_once('/').map(|(triple, _)| triple)
    }
}

/// Cargo's output profile directory name for one Oxide recipe.
fn profile_member(profile: Profile) -> &'static str {
    match profile {
        Profile::Quick | Profile::Debug => "debug",
        Profile::Release => "release",
    }
}

/// One component of a member name, without path meaning.
fn plain(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A command's complete profile-output set: sorted, duplicate-free and
/// prefix-free, so no member is a parent directory of another.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Declaration {
    members: Vec<Member>,
}

impl Declaration {
    /// Build a declaration from named members. Duplicates collapse; the
    /// prefix-free and total-cap rules are enforced here so no caller can
    /// publish a contradictory or oversized set.
    pub(crate) fn new(members: impl IntoIterator<Item = Member>) -> Result<Self, String> {
        let mut names: Vec<String> = members.into_iter().map(|member| member.name).collect();
        names.sort();
        names.dedup();
        validate_set(&names, MAX_MEMBERS)?;
        Ok(Self {
            members: names.into_iter().map(|name| Member { name }).collect(),
        })
    }

    pub(crate) fn empty() -> Self {
        Self {
            members: Vec::new(),
        }
    }

    pub(crate) fn members(&self) -> &[Member] {
        &self.members
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub(crate) fn contains(&self, member: &Member) -> bool {
        self.members.binary_search(member).is_ok()
    }

    /// The declared names, for a diagnostic.
    pub(crate) fn names(&self) -> String {
        if self.members.is_empty() {
            "none".to_owned()
        } else {
            self.members
                .iter()
                .map(Member::name)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

/// Validate one set of member names against the total cap and the
/// prefix-free rule. Names are already sorted and duplicate-free.
pub(crate) fn validate_set(names: &[String], cap: usize) -> Result<(), String> {
    if names.len() > cap {
        return Err(format!(
            "one command may write at most {cap} profile output members; it selected {}",
            names.len()
        ));
    }
    for pair in names.windows(2) {
        if prefix_of(&pair[0], &pair[1]) {
            return Err(format!(
                "the output members {:?} and {:?} overlap; declare only the deeper one",
                pair[0], pair[1]
            ));
        }
    }
    Ok(())
}

/// Whether `longer` is `shorter` plus one more path component.
fn prefix_of(shorter: &str, longer: &str) -> bool {
    longer
        .strip_prefix(shorter)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// A value Cargo accepts as a `--target` platform: one plain triple, not a
/// JSON specification or a path.
pub(crate) fn plain_triple(text: &str) -> bool {
    !text.is_empty()
        && !text.contains('/')
        && !text.ends_with(".json")
        && plain(text)
        && text != "."
        && text != ".."
}

/// One command's output-member classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Layout {
    /// These members are the complete profile-output set of this command.
    Declared(Declaration),
    /// The command's output directories cannot be determined, so a checkout
    /// with owned member links refuses before Cargo instead of guessing.
    Unknown(String),
}

/// Classify a leased command's profile outputs.
///
/// `targets` are every `--target` value before `--`, in order; `explicit`
/// says whether the caller named a profile. `bench` without `--profile`
/// builds Cargo's release-based `bench` profile, whose output directory is
/// still `release`; `bench --profile quick|debug` follows Cargo's `dev`
/// profile and therefore `debug`. `doc` and `rustdoc` compile the selected
/// profile's units and are classified the same way, while their own
/// `target/doc` directory is a real tool directory outside this set.
pub(crate) fn classify(
    command: &str,
    profile: Profile,
    explicit_profile: bool,
    targets: &[String],
) -> Layout {
    let bench_dev = command == "bench" && explicit_profile;
    let member = if bench_dev && profile != Profile::Release {
        "debug"
    } else if command == "bench" && !explicit_profile {
        "release"
    } else {
        profile_member(profile)
    };
    let mut members = Vec::new();
    if targets.is_empty() {
        members.push(Member {
            name: member.to_owned(),
        });
    } else {
        for target in targets {
            if !plain_triple(target) {
                return Layout::Unknown(format!(
                    "the target {target:?} is not a plain triple, so its profile output directory is not modelled; Oxide does not guess where a JSON target specification or a configured target writes"
                ));
            }
            members.push(Member {
                name: format!("{target}/{member}"),
            });
        }
    }
    match Declaration::new(members) {
        Ok(declaration) => Layout::Declared(declaration),
        Err(reason) => Layout::Unknown(reason),
    }
}

/// Parse repeated `--output <member>` declarations for `oxide exec`, in the
/// caller's order. The caller declares the entire profile-output set: an
/// undeclared member is a refusal in a checkout with owned member links, not
/// a guess.
pub(crate) fn declared(values: &[String]) -> Result<Declaration, String> {
    let members = values
        .iter()
        .map(|value| Member::parse(value))
        .collect::<Result<Vec<_>, _>>()?;
    Declaration::new(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str) -> Member {
        Member::parse(name).unwrap()
    }

    fn names(declaration: &Declaration) -> Vec<&str> {
        declaration.members().iter().map(Member::name).collect()
    }

    #[test]
    fn only_the_four_v1_forms_are_members() {
        for name in ["debug", "release", "aarch64-apple-darwin/debug"] {
            assert_eq!(member(name).name(), name);
        }
        assert_eq!(
            member("x86_64-unknown-linux-gnu/release").name(),
            "x86_64-unknown-linux-gnu/release"
        );
        for name in [
            "",
            "debug/",
            "/debug",
            "doc",
            "tmp",
            "debug/doc",
            "debug/release/debug",
            "./debug",
            "../debug",
            "debug/../release",
            "aarch64-apple-darwin/..",
            "a b/debug",
            "a b",
            "triple\\debug",
            "debug//release",
            "oxide-quick",
        ] {
            assert!(Member::parse(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn declarations_deduplicate_sort_and_stay_prefix_free() {
        let declaration = Declaration::new([
            member("release"),
            member("debug"),
            member("release"),
            member("aarch64-apple-darwin/debug"),
        ])
        .unwrap();
        assert_eq!(
            names(&declaration),
            ["aarch64-apple-darwin/debug", "debug", "release"]
        );
        let overlapping = Declaration::new([member("debug"), member("debug/debug")]);
        assert!(overlapping.is_err());
    }

    #[test]
    fn one_command_may_name_at_most_eight_members() {
        let eight = (0..8)
            .map(|index| format!("triple{index}/debug"))
            .collect::<Vec<_>>();
        let members = eight.iter().map(|name| member(name)).collect::<Vec<_>>();
        assert_eq!(Declaration::new(members).unwrap().members().len(), 8);
        let nine = (0..9)
            .map(|index| member(&format!("triple{index}/debug")))
            .collect::<Vec<_>>();
        assert!(Declaration::new(nine).is_err());
    }

    #[test]
    fn profiles_map_to_cargos_output_directories() {
        let host = |profile, explicit| {
            let layout = classify("build", profile, explicit, &[]);
            match layout {
                Layout::Declared(declaration) => declaration,
                Layout::Unknown(reason) => panic!("{reason}"),
            }
        };
        assert_eq!(names(&host(Profile::Quick, false)), ["debug"]);
        assert_eq!(names(&host(Profile::Debug, true)), ["debug"]);
        assert_eq!(names(&host(Profile::Release, true)), ["release"]);
        // Plain bench builds Cargo's release-based bench profile; an explicit
        // quick/debug bench follows dev, whose output directory is `debug`.
        let bench = |profile, explicit| match classify("bench", profile, explicit, &[]) {
            Layout::Declared(declaration) => names(&declaration)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            Layout::Unknown(reason) => panic!("{reason}"),
        };
        assert_eq!(bench(Profile::Quick, false), ["release"]);
        assert_eq!(bench(Profile::Release, true), ["release"]);
        assert_eq!(bench(Profile::Quick, true), ["debug"]);
        // `doc` compiles the selected profile's units too.
        assert_eq!(bench(Profile::Release, true), ["release"]);
        let doc = match classify("doc", Profile::Quick, false, &[]) {
            Layout::Declared(declaration) => names(&declaration)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            Layout::Unknown(reason) => panic!("{reason}"),
        };
        assert_eq!(doc, ["debug"]);
    }

    #[test]
    fn every_plain_target_expands_and_duplicates_collapse() {
        let targets = ["aarch64-apple-darwin", "aarch64-apple-darwin"]
            .map(str::to_owned)
            .to_vec();
        let Layout::Declared(declaration) = classify("build", Profile::Quick, false, &targets)
        else {
            panic!("a plain triple must classify");
        };
        assert_eq!(names(&declaration), ["aarch64-apple-darwin/debug"]);
    }

    #[test]
    fn json_specifications_and_paths_are_unknown() {
        for target in [
            "aarch64-apple-darwin.json",
            "targets/custom.json",
            "/abs/spec.json",
            "..",
            "a b",
            "",
        ] {
            assert!(matches!(
                classify("build", Profile::Quick, false, &[target.to_owned()]),
                Layout::Unknown(_)
            ));
        }
        assert!(plain_triple("x86_64-unknown-linux-gnu"));
        assert!(!plain_triple("x86_64-unknown-linux-gnu.json"));
    }

    #[test]
    fn exec_declarations_are_complete_and_bounded() {
        let declaration =
            declared(&["debug".to_owned(), "release".to_owned()]).expect("two members");
        assert_eq!(names(&declaration), ["debug", "release"]);
        assert!(declared(&["debug".to_owned(), "debug".to_owned()])
            .is_ok_and(|d| d.members().len() == 1));
        assert!(declared(&["custom".to_owned()]).is_err());
        assert!(declared(&["aarch64-apple-darwin/debug".to_owned()]).is_ok());
        assert!(declared(&["debug/debug".to_owned()]).is_ok());
        assert!(declared(&["debug".to_owned(), "debug/debug".to_owned()]).is_err());
        let nine = (0..9)
            .map(|index| format!("triple{index}/release"))
            .collect::<Vec<_>>();
        assert!(declared(&nine).is_err());
    }

    #[test]
    fn members_have_a_relative_target_path() {
        assert_eq!(
            member("aarch64-apple-darwin/release").relative(),
            std::path::PathBuf::from("aarch64-apple-darwin/release")
        );
        assert_eq!(member("debug").triple(), None);
        assert_eq!(
            member("aarch64-apple-darwin/debug").triple(),
            Some("aarch64-apple-darwin")
        );
    }
}
