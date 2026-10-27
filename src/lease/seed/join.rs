//! Bounded reader for rustc's makefile dep-info (`.d`), for the donation join
//! re-check of `oxide#137` §6.3 step 6.
//!
//! The catalog records one unit's join as the `.d`'s own SHA-256 plus the rule
//! targets and prerequisite paths it was derived from. Hashing the donated
//! `.d` proves the bytes did not change; re-parsing them here proves the
//! recorded targets and inputs really are that file's rules, so a catalog whose
//! join record is internally inconsistent is refused rather than trusted.
//!
//! The grammar accepted is rustc's ordinary output: one rule per logical line,
//! `target(s) : prerequisite(s)`, where a backslash escapes the next character
//! (so `\ `, `\:` and `\\` survive), `$$` is a literal `$`, and a trailing
//! backslash continues the line. Empty lines are ignored and an empty
//! prerequisite list is allowed (the phony source rules rustc writes). Anything
//! else is refused: this reader classifies, it never repairs.

/// One `.d` file's byte ceiling (§8.1: 1 MiB per `.d`).
pub const MAX_JOIN_BYTES: usize = 1024 * 1024;
/// Rule targets per `.d` (§8.1).
pub const MAX_JOIN_TARGETS: usize = 4096;
/// Prerequisite paths per `.d` (§8.1: 4,096 per unit).
pub const MAX_JOIN_INPUTS: usize = 4096;
/// One path string's byte ceiling.
pub const MAX_JOIN_PATH_BYTES: usize = 4096;

/// Why a `.d` could not be read as a join.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinRefusal {
    /// Larger than [`MAX_JOIN_BYTES`].
    TooLarge,
    /// Not UTF-8 text.
    NotUtf8,
    /// A rule, escape or token the grammar does not allow.
    Malformed,
    /// A rule or input count beyond its bound.
    Budget,
}

/// One rule of a `.d`: its targets and its prerequisites, in file order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinRule {
    pub targets: Vec<String>,
    pub inputs: Vec<String>,
}

/// The rules one `.d` records, plus their flat target and prerequisite sets in
/// first-seen order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedJoin {
    pub targets: Vec<String>,
    pub inputs: Vec<String>,
    pub rules: Vec<JoinRule>,
}

impl ParsedJoin {
    /// The rule that names `path` as one of its targets.
    pub fn rule_for(&self, path: &str) -> Option<&JoinRule> {
        self.rules
            .iter()
            .find(|rule| rule.targets.iter().any(|target| target == path))
    }

    /// Whether the flat rule sets are exactly the recorded ones. Rule order and
    /// repeats are not part of the recorded evidence.
    pub fn sets_match(&self, targets: &[String], inputs: &[String]) -> bool {
        fn set(values: &[String]) -> std::collections::BTreeSet<&str> {
            values.iter().map(String::as_str).collect()
        }
        set(&self.targets) == set(targets) && set(&self.inputs) == set(inputs)
    }

    /// The one rule whose target set is exactly this artifact, or `None` when
    /// the artifact has no rule, two rules name it, or a rule names it among
    /// several targets. Duplicate or conflicting evidence is refused before
    /// any flattened-set comparison: two rules for one artifact never
    /// establish which prerequisites produce it.
    pub fn sole_rule_for(&self, artifact: &str) -> Option<&JoinRule> {
        let mut found: Option<&JoinRule> = None;
        for rule in &self.rules {
            if rule.targets.iter().any(|target| target == artifact) {
                if found.is_some() || rule.targets.len() != 1 {
                    return None;
                }
                found = Some(rule);
            }
        }
        found
    }

    /// Whether `artifact` has its own single-target rule whose prerequisites
    /// are nonempty and all drawn from the recorded inputs. A flattened rule
    /// that names several targets, a duplicate or conflicting rule, or a rule
    /// with no source establishes nothing about that artifact.
    pub fn artifact_rule_proven(&self, artifact: &str, recorded_inputs: &[String]) -> bool {
        let inputs: std::collections::BTreeSet<&str> =
            recorded_inputs.iter().map(String::as_str).collect();
        self.sole_rule_for(artifact).is_some_and(|rule| {
            !rule.inputs.is_empty()
                && rule
                    .inputs
                    .iter()
                    .all(|input| inputs.contains(input.as_str()))
        })
    }
}

/// Parse one bounded `.d`.
pub fn parse(bytes: &[u8]) -> Result<ParsedJoin, JoinRefusal> {
    if bytes.len() > MAX_JOIN_BYTES {
        return Err(JoinRefusal::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| JoinRefusal::NotUtf8)?;
    let mut parsed = ParsedJoin {
        targets: Vec::new(),
        inputs: Vec::new(),
        rules: Vec::new(),
    };
    let mut logical = String::new();
    for line in text.split('\n') {
        if let Some(continued) = strip_continuation(line) {
            logical.push_str(continued);
            continue;
        }
        logical.push_str(line);
        flush_line(&mut logical, &mut parsed)?;
    }
    flush_line(&mut logical, &mut parsed)?;
    Ok(parsed)
}

/// A rule line continued by a single trailing backslash: that backslash is a
/// line continuation, while an even count is an escaped literal backslash.
fn strip_continuation(line: &str) -> Option<&str> {
    let trimmed = line.strip_suffix('\r').unwrap_or(line);
    let backslashes = trimmed.chars().rev().take_while(|ch| *ch == '\\').count();
    if backslashes % 2 == 1 {
        Some(&trimmed[..trimmed.len() - 1])
    } else {
        None
    }
}

fn flush_line(line: &mut String, parsed: &mut ParsedJoin) -> Result<(), JoinRefusal> {
    let text = std::mem::take(line);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let colon = first_unescaped_colon(trimmed).ok_or(JoinRefusal::Malformed)?;
    let targets = tokenize(trimmed[..colon].trim())?;
    if targets.is_empty() {
        return Err(JoinRefusal::Malformed);
    }
    let inputs = tokenize(trimmed[colon + 1..].trim())?;
    parsed.rules.push(JoinRule {
        targets: targets.clone(),
        inputs: inputs.clone(),
    });
    if parsed.rules.len() > MAX_JOIN_TARGETS {
        return Err(JoinRefusal::Budget);
    }
    parsed.targets.extend(targets);
    if parsed.targets.len() > MAX_JOIN_TARGETS {
        return Err(JoinRefusal::Budget);
    }
    parsed.inputs.extend(inputs);
    if parsed.inputs.len() > MAX_JOIN_INPUTS {
        return Err(JoinRefusal::Budget);
    }
    for path in parsed.targets.iter().chain(parsed.inputs.iter()) {
        if path.len() > MAX_JOIN_PATH_BYTES {
            return Err(JoinRefusal::Malformed);
        }
    }
    Ok(())
}

fn first_unescaped_colon(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b':' => return Some(index),
            _ => {}
        }
    }
    None
}

/// Split one side of a rule on unescaped blanks, honouring `\x` and `$$`.
fn tokenize(text: &str) -> Result<Vec<String>, JoinRefusal> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(next) => token.push(next),
                None => return Err(JoinRefusal::Malformed),
            },
            '$' => match chars.next() {
                Some('$') => token.push('$'),
                _ => return Err(JoinRefusal::Malformed),
            },
            ' ' | '\t' | '\r' => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            _ => token.push(ch),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    Ok(tokens)
}
