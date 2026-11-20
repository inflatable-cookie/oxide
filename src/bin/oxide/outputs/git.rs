//! Bounded Git probes and ignore-prerequisite checks for owned links.

use super::adopt::AUTO_ADOPT_MEMBERS;
use super::Context;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// One bounded Git probe's time limit.
const GIT_PROBE_TIME: Duration = Duration::from_secs(5);
/// Byte cap for one Git probe's stdout.
const MAX_GIT_PROBE_BYTES: usize = 64 * 1024;
/// Repository-addressing variables a probe removes: a first conversion asks
/// about this checkout's real work tree, never an inherited one.
const GIT_REPOSITORY_ENVIRONMENT: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
];

/// The first conversion's ignore prerequisite (oxide#125).
///
/// Before any receipt, root or link effect, positively establish this
/// checkout's real Git work tree and the exact repository-relative
/// non-directory link path, then ask Git itself about that path. The index
/// must not already track it, and Git's winning ignore rule for it must be
/// neither a negation nor a trailing-slash (directory-only) rule: an
/// existing real empty target lets Git answer for a directory, so the losing
/// answer would wrongly cover a link. Trailing-slash and unsupported rule
/// forms are refused even when that is a false negative. Missing Git, a
/// non-repository, a timeout, a command error and malformed, oversized or
/// ambiguous output all refuse: unknown is never permission.
/// The ignore prerequisite for one link path: the legacy whole-target link
/// (`suffix` `None`) or one profile member below `target/` (`suffix` is the
/// member). A profile member is a path inside the real `target` directory, so
/// a winning parent rule such as `/target/` genuinely ignores it and is
/// accepted; the legacy link is itself replaced, so a directory-only rule
/// cannot cover it.
pub(in crate::outputs) fn refuse_link_git_exposure_at(
    context: &Context,
    link: &Path,
    suffix: Option<&str>,
) -> Result<(), String> {
    let link = link.to_path_buf();
    let answer = first_link_git_answer(context, suffix);
    let Some(fix) = answer.fix.as_deref() else {
        let LinkGitVerdict::Unknown(reason) = &answer.verdict else {
            // A confirmed work tree always yields the exact rule to add.
            return Err("outputs: the ignore prerequisite answered without a fix".to_owned());
        };
        return Err(link_git_unknown_refusal(&link, reason, None));
    };
    let relative = answer.relative.as_deref().unwrap_or_default();
    match &answer.verdict {
        LinkGitVerdict::Eligible => Ok(()),
        LinkGitVerdict::Tracked => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git already tracks `{relative}`, so the published link would be a tracked path. Remove it from the index (`git rm --cached {relative}`) and {fix}; nothing was changed",
            link.display()
        )),
        LinkGitVerdict::Unignored => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git does not ignore `{relative}`, so the published link would appear as an untracked path. {fix}; nothing was changed",
            link.display()
        )),
        LinkGitVerdict::Negated(rule) => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git's winning rule `{}` ({}:{}) re-includes `{relative}`. {fix}; nothing was changed",
            link.display(),
            rule.pattern,
            rule.source,
            rule.line
        )),
        LinkGitVerdict::DirectoryOnly(rule) => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git's winning rule `{}` ({}:{}) matches directories only, so it would not cover the published link. {fix}; nothing was changed",
            link.display(),
            rule.pattern,
            rule.source,
            rule.line
        )),
        LinkGitVerdict::Unknown(reason) => Err(link_git_unknown_refusal(&link, reason, Some(fix))),
    }
}

/// One positive answer from the ignore prerequisite, with the exact rule to
/// add and the repository-relative link path Git's work-tree answer makes
/// available. Both are `None` only when the work tree itself could not be
/// confirmed.
#[derive(Debug)]
pub(in crate::outputs) struct LinkGitAnswer {
    pub(in crate::outputs) verdict: LinkGitVerdict,
    pub(in crate::outputs) relative: Option<String>,
    pub(in crate::outputs) fix: Option<String>,
    /// The repository `.gitignore` the fix names, when Git confirmed the work
    /// tree. `None` only with an unconfirmed work tree.
    pub(in crate::outputs) gitignore: Option<PathBuf>,
}

/// What Git positively established about one exact non-directory link path.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::outputs) enum LinkGitVerdict {
    /// Untracked, and its winning rule covers the link.
    Eligible,
    /// Git's index already tracks the path.
    Tracked,
    /// No rule ignores the path.
    Unignored,
    /// The winning rule re-includes the path.
    Negated(IgnoreRule),
    /// The winning rule matches directories only, so it never covers a link.
    DirectoryOnly(IgnoreRule),
    /// Git could not answer; unknown is never permission.
    Unknown(String),
}

/// The ignore prerequisite's answer for one link path (oxide#125): the legacy
/// whole-target link (`suffix` `None`) or one profile member below `target/`
/// (`suffix` is the member). A profile member is a path inside the real
/// `target` directory, so a winning parent rule such as `/target/` genuinely
/// ignores it and is accepted; the legacy link is itself replaced, so a
/// directory-only rule cannot cover it.
///
/// This is a positive answer rather than a message because three callers ask
/// the same question with different consequences: a conversion refuses
/// (oxide#125), an automatic first conversion treats it as ineligibility
/// (oxide#154), and `oxide init` names the exact rule to add without editing
/// anything (oxide#154).
pub(in crate::outputs) fn first_link_git_answer(
    context: &Context,
    suffix: Option<&str>,
) -> LinkGitAnswer {
    let (root, relative) = match git_link_path(context, suffix) {
        Ok(found) => found,
        Err(reason) => {
            return LinkGitAnswer {
                verdict: LinkGitVerdict::Unknown(reason),
                relative: None,
                fix: None,
                gitignore: None,
            }
        }
    };
    let fix = format!(
        "Ignore the non-directory path: add `/{relative}` (or `{relative}`), not `{relative}/`, to {}",
        root.join(".gitignore").display()
    );
    let verdict = first_link_git_verdict(&root, &relative, suffix.is_some())
        .unwrap_or_else(LinkGitVerdict::Unknown);
    LinkGitAnswer {
        verdict,
        relative: Some(relative),
        fix: Some(fix),
        gitignore: Some(root.join(".gitignore")),
    }
}

/// Ask Git about the exact non-directory path in a confirmed work tree. Every
/// error is an unknown answer, never permission.
fn first_link_git_verdict(
    root: &Path,
    relative: &str,
    directory_rule_covers: bool,
) -> Result<LinkGitVerdict, String> {
    // A tracked path would publish a tracked symlink once the link is there.
    let tracked = git_probe(root, &["ls-files", "-z", "--", relative], &[], true)?;
    if tracked.code != Some(0) {
        return Err(format!(
            "git ls-files did not answer ({})",
            exit_label(tracked.code)
        ));
    }
    if !tracked.stdout.is_empty() {
        return Ok(LinkGitVerdict::Tracked);
    }
    // The exact non-directory path, decided by Git itself: `--no-index`
    // reads the rules rather than the index (tracked is refused above), and
    // `-v` names the winning rule, whose form is part of the answer.
    let input = format!("{relative}\0");
    let check = git_probe(
        root,
        &["check-ignore", "--no-index", "-v", "-z", "--stdin"],
        input.as_bytes(),
        false,
    )?;
    let rule = match check.code {
        Some(0) => parse_ignore_rule(&check.stdout, relative)?,
        Some(1) => return Ok(LinkGitVerdict::Unignored),
        code => {
            return Err(format!(
                "git check-ignore did not answer ({})",
                exit_label(code)
            ))
        }
    };
    if rule.pattern.starts_with('!') {
        return Ok(LinkGitVerdict::Negated(rule));
    }
    if !directory_rule_covers && (rule.pattern.ends_with('/') || rule.pattern.ends_with("\\/")) {
        return Ok(LinkGitVerdict::DirectoryOnly(rule));
    }
    Ok(LinkGitVerdict::Eligible)
}

/// Why a published link would be unsafe for this exact path, as one sentence
/// without the fix. `None` when Git positively confirmed the path.
fn link_git_problem(answer: &LinkGitAnswer) -> Option<String> {
    let relative = answer.relative.as_deref().unwrap_or("target");
    Some(match &answer.verdict {
        LinkGitVerdict::Eligible => return None,
        LinkGitVerdict::Tracked => format!(
            "Git already tracks `{relative}`, so the published link would be a tracked path; remove it from the index with `git rm --cached {relative}`"
        ),
        LinkGitVerdict::Unignored => format!(
            "Git does not ignore `{relative}`, so the published link would appear as an untracked path"
        ),
        LinkGitVerdict::Negated(rule) => format!(
            "Git's winning rule `{}` ({}:{}) re-includes `{relative}`",
            rule.pattern, rule.source, rule.line
        ),
        LinkGitVerdict::DirectoryOnly(rule) => format!(
            "Git's winning rule `{}` ({}:{}) matches directories only, so it would not cover the published link",
            rule.pattern, rule.source, rule.line
        ),
        LinkGitVerdict::Unknown(reason) => format!(
            "Git could not confirm its decision for the exact non-directory path ({reason})"
        ),
    })
}

/// `oxide init`'s `.gitignore` diagnostic for a first conversion (oxide#154).
///
/// A first permanent-drive conversion publishes owned `<checkout>/target/<member>`
/// links only when Git ignores each exact repository-relative path, so this
/// asks the same prerequisite for the first member link the automatic
/// conversion would publish (`target/debug`) — the same question the creator
/// asks, which is why a directory-only rule such as `target/` that covers the
/// member is accepted and stays silent. Rather than rewrite a user's rule,
/// `oxide init` names the exact rule to add (for a nested workspace, its
/// repository-relative spelling). It is effect-free: the same bounded Git
/// probe the conversion uses, and no write of any kind. `None` when Git
/// positively confirmed the path, or when this checkout is not inside a Git
/// work tree at all (init adopts it outside Git, where nothing is ignored and
/// nothing is converted).
pub fn gitignore_diagnostic(workspace: &Path) -> Option<String> {
    let context = Context::new(None, workspace.to_path_buf());
    let link = context.link();
    let answer = first_link_git_answer(&context, Some(AUTO_ADOPT_MEMBERS[0]));
    let problem = link_git_problem(&answer)?;
    // No relative path or .gitignore means Git never confirmed a work tree.
    let member = answer.relative.as_deref()?;
    let gitignore = answer.gitignore.as_deref()?;
    // The rule that covers every member link this first conversion publishes:
    // the member's parent path (`/target`, or a nested `/rust/target`).
    let parent = member.rsplit_once('/').map_or(member, |(parent, _)| parent);
    Some(format!(
        "permanent-drive outputs will not convert {} yet: {problem}. Add `/{parent}` (or `{parent}`) to {}: it covers the `{member}` member link this first conversion publishes. `oxide init` leaves every .gitignore rule exactly as it is",
        link.display(),
        gitignore.display()
    ))
}

fn link_git_unknown_refusal(link: &Path, reason: &str, fix: Option<&str>) -> String {
    match fix {
        Some(fix) => format!(
            "outputs: refusing to convert {} to an owned link: Git could not confirm its decision for the exact non-directory path ({reason}), which the first conversion needs. {fix}; nothing was changed",
            link.display()
        ),
        None => format!(
            "outputs: refusing to convert {} to an owned link: Git could not confirm this checkout's work tree and the exact non-directory path to ignore ({reason}), which the first conversion needs. Nothing was changed",
            link.display()
        ),
    }
}

fn exit_label(code: Option<i32>) -> String {
    code.map_or_else(|| "by signal".to_owned(), |code| format!("exit {code}"))
}

/// The real work tree and the exact repository-relative path of this
/// checkout's owned link: positively inside that work tree, no `..`, no
/// escape and no trailing slash.
fn git_link_path(context: &Context, suffix: Option<&str>) -> Result<(PathBuf, String), String> {
    let workspace = fs::canonicalize(&context.workspace)
        .map_err(|error| format!("cannot resolve {}: {error}", context.workspace.display()))?;
    let probe = git_probe(&workspace, &["rev-parse", "--show-toplevel"], &[], false)?;
    if probe.code != Some(0) {
        return Err(format!(
            "git rev-parse did not confirm a work tree ({})",
            exit_label(probe.code)
        ));
    }
    let text = std::str::from_utf8(&probe.stdout)
        .map_err(|_| "Git reported a work tree root that is not UTF-8".to_owned())?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() || text.contains('\n') {
        return Err("Git reported an empty or multiple work tree roots".to_owned());
    }
    let root = fs::canonicalize(text)
        .map_err(|error| format!("cannot resolve the Git work tree root {text}: {error}"))?;
    let relative = workspace
        .strip_prefix(&root)
        .map_err(|_| "the checkout is not inside the Git work tree Git reported".to_owned())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("the checkout path is not a plain relative path".to_owned());
        };
        let name = name
            .to_str()
            .ok_or("the checkout path is not valid UTF-8")?;
        if name.is_empty() || name.contains('/') || name.contains('\0') {
            return Err("the checkout path has an unusable component".to_owned());
        }
        parts.push(name.to_owned());
    }
    parts.push("target".to_owned());
    if let Some(suffix) = suffix {
        // The member is parsed once: its name must be the suffix exactly, and
        // every component is then copied verbatim.
        if !crate::layout::Member::parse(suffix).is_ok_and(|member| member.name() == suffix) {
            return Err("the profile member is not a plain relative path".to_owned());
        }
        for component in suffix.split('/') {
            if component.is_empty() {
                return Err("the profile member is not a plain relative path".to_owned());
            }
            parts.push(component.to_owned());
        }
    }
    Ok((root, parts.join("/")))
}

/// One winning ignore rule from `git check-ignore --no-index -v -z --stdin`:
/// `<source>\0<line>\0<pattern>\0<pathname>\0`.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::outputs) struct IgnoreRule {
    pub(in crate::outputs) source: String,
    pub(in crate::outputs) line: u64,
    pub(in crate::outputs) pattern: String,
}

pub(in crate::outputs) fn parse_ignore_rule(
    output: &[u8],
    expected: &str,
) -> Result<IgnoreRule, String> {
    let fields: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    // Exactly one complete record, ending with its NUL separator.
    if fields.len() != 5 || !fields[4].is_empty() {
        return Err("Git's verbose ignore output is not one complete record".to_owned());
    }
    let text = |index: usize, what: &str| -> Result<&str, String> {
        let bytes = fields[index];
        if bytes.is_empty() || bytes.contains(&b'\n') || bytes.contains(&b'\0') {
            return Err(format!("Git's ignore output has an unusable {what}"));
        }
        std::str::from_utf8(bytes)
            .map_err(|_| format!("Git's ignore output has a non-UTF-8 {what}"))
    };
    let source = text(0, "rule source")?;
    let line = text(1, "rule line")?;
    let pattern = text(2, "rule pattern")?;
    let pathname = text(3, "pathname")?;
    if pathname != expected {
        return Err("Git answered for another path".to_owned());
    }
    if !line.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Git's ignore output has an unusable rule line".to_owned());
    }
    let line = line
        .parse::<u64>()
        .map_err(|_| "Git's ignore output has an unusable rule line".to_owned())?;
    Ok(IgnoreRule {
        source: source.to_owned(),
        line,
        pattern: pattern.to_owned(),
    })
}

/// One bounded, effect-free Git call in `dir`, with `input` on stdin.
/// Optional locks and prompts are off, repository overrides are removed,
/// output is capped and a child that outlives the probe is killed. Every
/// failure is an error, never an answer. `literal_pathspecs` makes `--` paths
/// literal for `ls-files`; commands that reject that magic (check-ignore's
/// stdin paths) are called without it.
fn git_probe(
    dir: &Path,
    args: &[&str],
    input: &[u8],
    literal_pathspecs: bool,
) -> Result<GitProbe, String> {
    let name = args.first().copied().unwrap_or("git");
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(["-c", "submodule.recurse=0", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Its own process group, so a grandchild that inherits the pipes is
        // stopped with the probe instead of outliving it.
        .process_group(0);
    for variable in GIT_REPOSITORY_ENVIRONMENT {
        command.env_remove(variable);
    }
    // Inherited config injection could change the ignore decision (for
    // example `core.excludesFile`) from the one a plain `git status` in the
    // same checkout would make.
    for (name, _) in std::env::vars_os() {
        let Some(name) = name.to_str() else { continue };
        if name == "GIT_CONFIG_COUNT"
            || name == "GIT_CONFIG_PARAMETERS"
            || name.starts_with("GIT_CONFIG_KEY_")
            || name.starts_with("GIT_CONFIG_VALUE_")
        {
            command.env_remove(name);
        }
    }
    if literal_pathspecs {
        command.env("GIT_LITERAL_PATHSPECS", "1");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot run git {name}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("git {name} kept no stdout"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout
            .take(MAX_GIT_PROBE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(read);
    });
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let deadline = Instant::now() + crate::budget::clamp(GIT_PROBE_TIME);
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                crate::describe::stop_group_and_reap(&mut child);
                return Err(format!("git {name} did not finish in time"));
            }
            Err(error) => {
                crate::describe::stop_group_and_reap(&mut child);
                return Err(format!("cannot wait for git {name}: {error}"));
            }
        }
    };
    // The output shares the one deadline the child was given: a grandchild
    // that keeps the pipe open cannot extend the caller's wait past
    // `GIT_PROBE_TIME` in total.
    let settle = deadline.saturating_duration_since(Instant::now());
    let bytes = receiver
        .recv_timeout(settle)
        .map_err(|_| {
            crate::describe::stop_group_and_reap(&mut child);
            format!("git {name} did not close its output in time")
        })?
        .map_err(|error| format!("cannot read git {name} output: {error}"))?;
    if bytes.len() > MAX_GIT_PROBE_BYTES {
        return Err(format!("git {name} output is over its byte cap"));
    }
    Ok(GitProbe {
        code,
        stdout: bytes,
    })
}

/// One bounded Git probe's answer; a non-zero exit is an answer, not an error.
pub(in crate::outputs) struct GitProbe {
    pub(in crate::outputs) code: Option<i32>,
    pub(in crate::outputs) stdout: Vec<u8>,
}

/// Whether Git would track a path inside the checkout. A checkout that is
/// not a work tree has no tracking risk.
///
/// Both questions go through the same bounded, hardened probe the first
/// conversion uses: an inherited `GIT_DIR`/`GIT_WORK_TREE` or config injection
/// can never redirect the answer, optional locks and prompts are off, and the
/// probe is time and byte capped. Every failure refuses: unknown is never
/// permission, so a spawn error or an unusable `rev-parse` answer never
/// pretends the path is safe.
pub(in crate::outputs) fn git_would_track(checkout: &Path, relative: &str) -> Result<bool, String> {
    git_would_track_with(|args| git_probe(checkout, args, &[], false), relative)
}

/// The decision half of [`git_would_track`], with the probe injectable so the
/// fail-closed answers are exercisable without a broken Git installation.
pub(in crate::outputs) fn git_would_track_with(
    probe: impl Fn(&[&str]) -> Result<GitProbe, String>,
    relative: &str,
) -> Result<bool, String> {
    let check = probe(&["check-ignore", "-q", "--", relative])?;
    match check.code {
        // A winning ignore rule: Git would not track the path.
        Some(0) => Ok(false),
        // Nothing ignores it: inside a work tree Git would track it.
        Some(1) => Ok(true),
        // Any other answer is unknown, and only a positively non-work-tree
        // checkout makes unknown safe. A spawn failure above and an
        // unusable `rev-parse` answer here both propagate as refusals.
        _ => {
            let inside = probe(&["rev-parse", "--is-inside-work-tree"])?;
            match inside.code {
                Some(0) if String::from_utf8_lossy(&inside.stdout).trim() == "true" => Err(
                    "git could not decide whether the local output root is ignored; refusing to create it"
                        .to_owned(),
                ),
                Some(_) => Ok(false),
                None => Err(
                    "git could not decide whether the local output root is ignored; refusing to create it"
                        .to_owned(),
                ),
            }
        }
    }
}
