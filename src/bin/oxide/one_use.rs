//! One-use workspaces: classification (design §3) and the guarded
//! local-output branch's decision text (design §4.1-§4.5).
//!
//! Owning contract: `docs/knowledge/contracts/one-use-workspaces.md`, slice
//! **S1** of §15 (decision `dd090b31`). This module owns exactly the part S1
//! names: which workspace is one-use, the two predicates (class versus guard),
//! the bounded reasons a candidate root is dropped, and the exact diagnostics
//! the guard prints. The receipt/path guard itself lives in `outputs`, where
//! the receipt reader, the link classifier and the binding custody already
//! are; the slot record's class belongs to S2 and is deliberately absent.
//!
//! # Dormancy (temporary)
//!
//! The feature lands **dormant**: production behaviour is byte-identical to
//! the release before it until the Elevated proof slice activates it. The one
//! switch is [`is_active`], a process-local flag that starts `false` and that
//! only unit tests set (through [`with_active`]). No environment variable, no
//! file and no persisted state can turn it on, so a release binary has no
//! activation path at all. **This switch is temporary**: S4 of the same
//! contract replaces it with the non-default `proof-controls` feature's two
//! readers (`OXIDE_TEST_WORKSPACE_CLASS`, `OXIDE_TEST_TEMP_ROOTS`), which are
//! the controls §11.1 defines for the proof slice. Until then nothing reads
//! the production opt-in either, because classification is only computed when
//! the switch is on.
//!
//! Classification reads no receipt, no slot, no child temporary directory and
//! no payload: it is four candidate roots, the account home, the registered
//! pool facts this command already selected, and the opt-in value.

use crate::removable;
use crate::wrapper;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The one production opt-in (design §3.5): exactly `1`.
pub(crate) const OPT_IN: &str = "OXIDE_ONE_USE_WORKSPACE";
/// The qualified shared proof supervisor's scratch root: a candidate source,
/// never a classifier of its own (design §3.2).
pub(crate) const TASK_SCRATCH: &str = "OXIDE_TASK_SCRATCH_ROOT";
/// The command-line environment's temporary directory: a candidate source.
pub(crate) const TMPDIR: &str = "TMPDIR";

/// The two literal macOS temporary roots (design §3.2, sources 1 and 2). The
/// user default `TMPDIR` canonicalizes into the second one, which is why the
/// literal form is listed.
const LITERAL_ROOTS: [&str; 2] = ["/private/tmp", "/private/var/folders"];

/// The exact §4.4 diagnostic for an unresolved `Prepared` transition in a
/// guarded workspace, with no adopt or detach repair advice.
pub(crate) const PREPARED_GUARDED: &str = "prepared output transition unresolved; this guarded workspace cannot replay it; nothing changed";

/// The bounded refusal of a checkout whose clean barrier never became
/// available inside the guarded verbs' attempt budget (design §4.1 step 4).
pub(crate) const BARRIER_BUSY: &str = "clean barrier busy: the checkout stayed busy for the whole bounded attempt budget; nothing was changed; retry once the other oxide commands in this checkout finish";

/// The bounded refusal of a dead busy fact (design §4.1 step 5): a barrier
/// over it could never make progress, so the guarded verb refuses instead of
/// waiting.
pub(crate) fn barrier_dead(what: &str) -> String {
    format!(
        "clean barrier refused: {what}, so a barrier would never make progress; nothing was changed; an ordinary oxide command in this checkout recovers it"
    )
}

/// The bounded refusal of an unreadable or unclassifiable busy fact (design
/// §4.1 step 5): unknown custody is protected.
pub(crate) fn barrier_unknown(what: &str) -> String {
    format!("clean barrier refused: {what}; nothing was changed and the unknown custody is kept")
}

/// The general guarded `adopt` refusal for every receipt state that is not a
/// `Prepared` transition (design §4.5).
pub(crate) fn adopt_refusal(workspace: &Path) -> String {
    format!(
        "outputs adopt refused: {} is a one-use (temporary) workspace, so it cannot own an output root and final outputs stay local; nothing was changed",
        wrapper::shown(&workspace.display().to_string())
    )
}

/// The guarded `Committed` refusal (design §4.2): a committed receipt selects
/// an owned root, so a guarded workspace never builds locally through it. The
/// one safe verb is named.
pub(crate) fn committed_refusal(root: &Path) -> String {
    format!(
        "outputs: this one-use (temporary) workspace cannot build through the owned root {} that its committed receipt selects; final outputs stay local. Run `oxide outputs detach` here to restore real directories, or build outside the temporary root; nothing was changed",
        wrapper::shown(&root.display().to_string())
    )
}

/// The one line a guarded `oxide outputs plan` prints: a bounded local plan,
/// never an adopt recommendation (design §4.5).
pub(crate) const PLAN_LOCAL: &str =
    "one-use (temporary) workspace: final outputs are local and no owned output root is selected";

/// The one bounded line `oxide status` appends for a guarded checkout (design
/// §4.5, §10.5). No savings claim.
pub(crate) const STATUS_LOCAL: &str =
    "outputs: this checkout is a one-use (temporary) workspace; final outputs are local and no owned output root is selected";

/// The activation switch. `false` in every production binary: see the module
/// documentation's dormancy note.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the one-use feature is active in this process. `false` everywhere
/// except a unit test that called [`with_active`].
pub(crate) fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Run `body` with the one-use feature active (tests only), restoring the
/// previous value afterwards. The switch is process-local rather than
/// thread-local because the guard, the classification and a test's assertions
/// must observe the same value; the unit suite is the only caller and it
/// serialises on the same value.
#[cfg(test)]
pub(crate) fn with_active<T>(body: impl FnOnce() -> T) -> T {
    let previous = ACTIVE.swap(true, Ordering::Relaxed);
    let result = body();
    ACTIVE.store(previous, Ordering::Relaxed);
    result
}

/// The registered pool facts the unsafe-broad test needs (design §3.3 item
/// 4). Unknown facts never justify guessing a safe root: an unreadable
/// registration drops every candidate root.
#[derive(Clone, Debug)]
pub(crate) enum Pools {
    Known(Vec<PathBuf>),
    Unknown(String),
}

/// The facts one classification reads. Every field is already-read input, so
/// tests can drive the table without an environment.
#[derive(Clone, Debug)]
pub(crate) struct Facts {
    /// Candidate roots in the design's source order, exactly as read.
    pub(crate) candidates: Vec<OsString>,
    /// The account home this command sees, and `$HOME` when it differs. Both
    /// are unsafe-broad roots.
    pub(crate) homes: Vec<PathBuf>,
    /// The payload roots of the pools this command selected, or why they
    /// cannot be read.
    pub(crate) pools: Pools,
    /// The raw opt-in value.
    pub(crate) opt_in: Option<OsString>,
}

impl Facts {
    /// The production facts: the four candidate sources, the account home and
    /// the registered pool payloads, all read from this command's environment
    /// and from `control` (the internal pool's control directory) without an
    /// effect. A missing pool contributes no payload root; an unreadable
    /// registration makes the pool facts unknown.
    pub(crate) fn from_control(control: Option<&Path>) -> Self {
        let mut candidates: Vec<OsString> = LITERAL_ROOTS.iter().map(OsString::from).collect();
        for name in [TMPDIR, TASK_SCRATCH] {
            if let Some(value) = std::env::var_os(name) {
                candidates.push(value);
            }
        }
        let config = crate::config::Facts::from_env();
        let mut homes = Vec::new();
        for home in [config.home.as_deref(), config.env_home.as_deref()]
            .into_iter()
            .flatten()
        {
            let home = PathBuf::from(home);
            if !homes.contains(&home) {
                homes.push(home);
            }
        }
        let pools = match control {
            None => Pools::Known(Vec::new()),
            Some(control) => {
                let Some(pool) = control.parent() else {
                    return Self {
                        candidates,
                        homes,
                        pools: Pools::Unknown(
                            "the internal pool's control directory has no parent".to_owned(),
                        ),
                        opt_in: std::env::var_os(OPT_IN),
                    };
                };
                let mut roots = vec![pool.join("payload")];
                match removable::read_registration(control) {
                    Ok(Some(registration)) => {
                        roots.push(registration.mount_point.clone());
                        roots.push(registration.mount_point.join(removable::PAYLOAD_DIR));
                    }
                    Ok(None) => {}
                    Err(reason) => {
                        return Self {
                            candidates,
                            homes,
                            pools: Pools::Unknown(reason),
                            opt_in: std::env::var_os(OPT_IN),
                        }
                    }
                }
                Pools::Known(roots)
            }
        };
        Self {
            candidates,
            homes,
            pools,
            opt_in: std::env::var_os(OPT_IN),
        }
    }
}

/// One command's workspace classification (design §3.6).
///
/// `membership` is the accepted root the canonical workspace is a strict
/// component descendant of; `opt_in` is the exact opt-in value. The two
/// predicates are deliberately separate: the admission **class** is
/// `one_use` and the local-output **guard** is `guarded`. In S1 both are true
/// for the same inputs, because the class-forcing control that separates them
/// (§11.1's `OXIDE_TEST_WORKSPACE_CLASS=normal`) belongs to S4; the methods
/// stay separate so that control can only ever clear `one_use`, never
/// `guarded`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Classification {
    membership: Option<PathBuf>,
    opt_in: bool,
    /// Bounded single-line reports, one per dropped root and one for an
    /// ignored opt-in value. Printed once, only while the feature is active.
    pub(crate) notes: Vec<String>,
}

impl Classification {
    /// The admission class (§3.6).
    pub(crate) fn one_use(&self) -> bool {
        self.membership.is_some() || self.opt_in
    }

    /// The local-output guard (§3.6). Implied by membership alone, so a caller
    /// that forces the class to `normal` still cannot disable it.
    pub(crate) fn guarded(&self) -> bool {
        self.membership.is_some() || self.one_use()
    }

    /// The bounded reason printed in the guarded command's one note.
    pub(crate) fn reason(&self) -> String {
        match (&self.membership, self.opt_in) {
            (Some(root), _) => format!(
                "this workspace is under the temporary root {}",
                wrapper::shown(&root.display().to_string())
            ),
            (None, true) => format!("{OPT_IN}=1"),
            (None, false) => "not a one-use workspace".to_owned(),
        }
    }
}

/// Classify one canonical workspace root (design §3.1-§3.5). Effect-free: the
/// only reads are `fs::canonicalize`, `symlink_metadata` for the directory
/// test and the environment facts already gathered.
pub(crate) fn classify(workspace: &Path, facts: &Facts) -> Classification {
    let mut notes = Vec::new();
    let accepted = accepted_roots(facts, &mut notes);
    // Both sides of the component test are canonical. A workspace that does
    // not resolve cannot be proved a descendant of any accepted root, so it
    // stays normal: the conservative direction (§3.3).
    let canonical = workspace.canonicalize().ok();
    let membership = canonical
        .as_deref()
        .and_then(|workspace| descendant(workspace, &accepted));
    let opt_in = match facts.opt_in.as_deref() {
        None => false,
        Some(value) if value == "1" => true,
        Some(value) => {
            notes.push(format!(
                "{OPT_IN}={} is ignored; only the exact value 1 opts a workspace in",
                wrapper::shown(&value.to_string_lossy())
            ));
            false
        }
    };
    Classification {
        membership,
        opt_in,
        notes,
    }
}

/// The accepted roots of one classification, with one bounded note per dropped
/// candidate (design §3.3 step 5).
fn accepted_roots(facts: &Facts, notes: &mut Vec<String>) -> Vec<PathBuf> {
    let mut accepted: Vec<PathBuf> = Vec::new();
    for raw in &facts.candidates {
        let Some(root) = accept_root(raw, facts, notes) else {
            continue;
        };
        if !accepted.contains(&root) {
            accepted.push(root);
        }
    }
    accepted
}

/// One candidate root: absolute, resolvable to an existing directory, not
/// unsafe-broad. Every rejection is reported once, with its reason escaped.
fn accept_root(raw: &OsStr, facts: &Facts, notes: &mut Vec<String>) -> Option<PathBuf> {
    let shown = || wrapper::shown(&Path::new(raw).display().to_string());
    let drop = |reason: &str, notes: &mut Vec<String>| {
        notes.push(format!("one-use root skipped: {} ({reason})", shown()));
        None
    };
    if raw.is_empty() {
        return drop("an empty path", notes);
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        // Never interpreted against the working directory (§3.3 step 1).
        return drop("not an absolute path", notes);
    }
    let Ok(root) = path.canonicalize() else {
        return drop("does not resolve to an existing directory", notes);
    };
    if !root.is_dir() {
        return drop("not a directory", notes);
    }
    if root == Path::new("/") {
        return drop("the filesystem root", notes);
    }
    if volume_root(&root) {
        return drop("a volume root", notes);
    }
    for home in &facts.homes {
        if home.canonicalize().ok().as_deref() == Some(root.as_path()) {
            return drop("the account home", notes);
        }
    }
    match &facts.pools {
        Pools::Unknown(reason) => {
            return drop(
                &format!(
                    "the registered pool layout is unavailable ({})",
                    wrapper::shown(reason)
                ),
                notes,
            )
        }
        Pools::Known(pools) => {
            for pool in pools {
                // A payload that does not resolve is compared as recorded: it
                // is still the truthful location of an absent root.
                let pool = pool.canonicalize().unwrap_or_else(|_| pool.clone());
                if root.starts_with(&pool) && root != pool {
                    return drop("inside a registered pool", notes);
                }
                if pool.starts_with(&root) && pool != root {
                    return drop("contains a registered pool payload", notes);
                }
            }
        }
    }
    Some(root)
}

/// Exactly one component below `/Volumes` (design §3.3 item 4), including a
/// mounted drive's root.
fn volume_root(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::RootDir))
        && components
            .next()
            .is_some_and(|component| component == Component::Normal(OsStr::new("Volumes")))
        && components
            .next()
            .is_some_and(|component| matches!(component, Component::Normal(_)))
        && components.next().is_none()
}

/// Strict component descendant (design §3.4): the root itself never matches,
/// `/private/tmpfoo` is not under `/private/tmp`, and `..` cannot appear
/// because both sides are canonical.
fn descendant(workspace: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .find(|root| {
            workspace
                .strip_prefix(root.as_path())
                .is_ok_and(|rest| !rest.as_os_str().is_empty())
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(label: &str) -> PathBuf {
        let base = std::env::var_os(TASK_SCRATCH)
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-one-use-{label}-{}-{}",
            std::process::id(),
            crate::process::now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::canonicalize(root).unwrap()
    }

    fn facts(candidates: Vec<&Path>, opt_in: Option<&str>) -> Facts {
        Facts {
            candidates: candidates
                .into_iter()
                .map(|path| path.as_os_str().to_owned())
                .collect(),
            homes: Vec::new(),
            pools: Pools::Known(Vec::new()),
            opt_in: opt_in.map(OsString::from),
        }
    }

    /// The classification table of §16.1: canonical membership, the root
    /// itself, a string-prefix neighbour, dedup, aliases and the two opt-in
    /// values. Also the class-versus-guard matrix of §3.6.
    #[test]
    fn the_classification_table_holds() {
        let here = scratch("table");
        let root = here.join("tmp");
        let workspace = root.join("repo");
        let neighbour = here.join("tmpfoo");
        for path in [&root, &workspace, &neighbour] {
            std::fs::create_dir_all(path).unwrap();
        }
        let root = std::fs::canonicalize(&root).unwrap();
        let workspace = std::fs::canonicalize(&workspace).unwrap();

        let classification = classify(&workspace, &facts(vec![&root], None));
        assert!(classification.guarded() && classification.one_use());
        assert!(classification
            .reason()
            .contains(&root.display().to_string()));

        // A command whose workspace IS the accepted root is normal.
        let at_root = classify(&root, &facts(vec![&root], None));
        assert!(!at_root.guarded() && !at_root.one_use());

        // `/private/tmpfoo` is not under `/private/tmp`: components, not
        // bytes.
        let outside = classify(&neighbour, &facts(vec![&root], None));
        assert!(!outside.guarded());

        // Dedup: the same real directory twice is one accepted root, so a
        // duplicate candidate reports nothing.
        let deduped = classify(&workspace, &facts(vec![&root, &root], None));
        assert!(deduped.notes.is_empty(), "{:?}", deduped.notes);

        // A symlinked alias resolves to the accepted root.
        let alias = here.join("alias");
        symlink(&root, &alias).unwrap();
        let through_alias = classify(&workspace, &facts(vec![&alias], None));
        assert!(through_alias.guarded());

        // Both opt-in values: exactly 1 opts in, everything else is ignored
        // and reported once, and is never a refusal.
        let opted = classify(&neighbour, &facts(Vec::new(), Some("1")));
        assert!(opted.guarded() && opted.one_use() && opted.notes.is_empty());
        for value in ["0", "true", "", "one-use"] {
            let ignored = classify(&neighbour, &facts(Vec::new(), Some(value)));
            assert!(!ignored.one_use() && !ignored.guarded(), "{value:?}");
            assert_eq!(ignored.notes.len(), 1, "{value:?}: {:?}", ignored.notes);
            assert!(
                ignored.notes[0].contains("is ignored"),
                "{:?}",
                ignored.notes
            );
        }

        // Opt-in adds one-use; it never removes membership.
        let both = classify(&workspace, &facts(vec![&root], Some("1")));
        assert!(both.guarded() && both.one_use());

        std::fs::remove_dir_all(&here).unwrap();
    }

    /// Every unsafe-broad root is dropped with one bounded note, and dropping
    /// one root never drops the others (§3.3 step 5).
    #[test]
    fn unsafe_broad_roots_are_dropped_individually() {
        let here = scratch("broad");
        let safe = here.join("safe");
        let workspace = safe.join("repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let home = here.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let pool = here.join("pool");
        let payload = pool.join("payload");
        std::fs::create_dir_all(payload.join("slot")).unwrap();
        // A mounted drive's root is exactly one component below the literal
        // `/Volumes`, which a fixture cannot create inside its own scratch.
        assert!(volume_root(Path::new("/Volumes/Builds")));
        assert!(!volume_root(Path::new("/Volumes")));
        assert!(!volume_root(Path::new("/Volumes/Builds/leaf")));

        let mut facts = facts(
            vec![Path::new("/"), &home, &pool, &payload.join("slot"), &safe],
            None,
        );
        facts.homes = vec![home.clone()];
        facts.pools = Pools::Known(vec![payload.clone()]);
        let classification = classify(&workspace, &facts);
        assert!(classification.guarded());
        let notes = classification.notes.join("\n");
        for reason in [
            "the filesystem root",
            "the account home",
            "contains a registered pool payload",
            "inside a registered pool",
        ] {
            assert!(notes.contains(reason), "{reason}: {notes}");
        }
        assert_eq!(classification.notes.len(), 4, "{notes}");
        // The safe root still applied, so membership came from it.
        assert!(classification.reason().contains("safe"));

        std::fs::remove_dir_all(&here).unwrap();
    }

    /// Relative, empty, absent and unreadable inputs are dropped with a
    /// bounded reason; unknown pool facts drop every root (§3.3).
    #[test]
    fn unusable_inputs_are_dropped() {
        let here = scratch("unusable");
        let workspace = here.join("repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let missing = here.join("absent");
        let file = here.join("file");
        std::fs::write(&file, b"x").unwrap();

        let mut facts = facts(
            vec![Path::new("relative/tmp"), &missing, &file, &here],
            None,
        );
        facts.candidates.insert(0, OsString::new());
        let classification = classify(&workspace, &facts);
        assert!(classification.guarded(), "the real root still applied");
        let notes = classification.notes.join("\n");
        for reason in [
            "an empty path",
            "not an absolute path",
            "does not resolve to an existing directory",
            "not a directory",
        ] {
            assert!(notes.contains(reason), "{reason}: {notes}");
        }

        // Unreadable registration: a safe root is not assumed.
        let mut unknown = facts.clone();
        unknown.pools = Pools::Unknown("the registration is unreadable".to_owned());
        let dropped = classify(&workspace, &unknown);
        assert!(!dropped.guarded());
        assert!(dropped.notes.join("\n").contains("unavailable"));

        std::fs::remove_dir_all(&here).unwrap();
    }

    /// Dormancy: nothing activates the feature by itself, and the switch is
    /// the only thing tests and the later proof slice set.
    #[test]
    fn the_feature_is_dormant_by_default() {
        assert!(!is_active());
        let here = scratch("dormant");
        let workspace = here.join("repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let classification = classify(&workspace, &facts(vec![&here], None));
        assert!(classification.guarded(), "classification itself is pure");
        assert!(!is_active(), "classification never activates the feature");
        assert!(with_active(is_active));
        assert!(!is_active());
        std::fs::remove_dir_all(&here).unwrap();
    }
}
