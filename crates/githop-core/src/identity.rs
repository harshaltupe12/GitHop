//! Effective author/committer identity via `git var` (Plan §8 C8, C9).
//!
//! CONTRACT FILE: types fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.
//!
//! Two ways in:
//! - [`effective_identity`] (no snapshot at hand): asks git itself in strict
//!   mode — `git var GIT_AUTHOR_IDENT` and `git var GIT_COMMITTER_IDENT`, the
//!   exact rules `git commit` applies. Up to 2 spawns.
//! - [`effective_identity_with`] (hooks, scan — a config snapshot was already
//!   read): one `git var -l` spawn. The config part of that output is skipped
//!   by **line count derived from the snapshot**, so multi-line config values
//!   (`alias.x = "…\nGIT_AUTHOR_IDENT=…"`) or env values printed after the idents
//!   (`GIT_PAGER`, `GIT_EDITOR`) can never be mistaken for the real idents
//!   (review 2026-10-31 #10). Strict validity is re-checked from the snapshot.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::Git;
use crate::gitconfig::ConfigSnapshot;

/// A name + email pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// Case-insensitive email comparison.
    pub fn email_matches(&self, email: &str) -> bool {
        self.email.trim().eq_ignore_ascii_case(email.trim())
    }
}

/// The identities the next commit would use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveIdentity {
    pub author: Identity,
    pub committer: Identity,
}

/// Parse `Name <email> 1730000000 +0100` (timestamp/tz optional).
///
/// Uses git's own split rule: the email starts after the first `<` and ends at
/// the first `>` after it. Name and email are trimmed.
pub fn parse_ident(s: &str) -> Option<Identity> {
    let lt = s.find('<')?;
    let gt = lt + 1 + s[lt + 1..].find('>')?;
    Some(Identity {
        name: s[..lt].trim().to_string(),
        email: s[lt + 1..gt].trim().to_string(),
    })
}

const AUTHOR_VAR: &str = "GIT_AUTHOR_IDENT=";
const COMMITTER_VAR: &str = "GIT_COMMITTER_IDENT=";

fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// Heuristic parse of `git var -l` output **without** a config snapshot.
///
/// git prints the config list, then its logical variables in a fixed order:
/// `GIT_COMMITTER_IDENT`, `GIT_AUTHOR_IDENT`, `GIT_EDITOR`,
/// `GIT_SEQUENCE_EDITOR`, `GIT_PAGER`, `GIT_DEFAULT_BRANCH`, … (verified with
/// Git for Windows 2.51). This takes the **first** `GIT_COMMITTER_IDENT=` line
/// directly followed by a `GIT_AUTHOR_IDENT=` line, which defeats values
/// printed *after* the idents (a multi-line `GIT_PAGER`/`GIT_EDITOR`), but a
/// multi-line *config* value could still fake the pair. Prefer
/// [`parse_var_list_anchored`]. `None` unless both idents are found and parse.
pub fn parse_var_list(stdout: &str) -> Option<EffectiveIdentity> {
    let lines: Vec<&str> = stdout.split('\n').map(strip_cr).collect();
    lines.windows(2).find_map(|w| {
        let committer = w[0].strip_prefix(COMMITTER_VAR)?;
        let author = w[1].strip_prefix(AUTHOR_VAR)?;
        Some(EffectiveIdentity {
            author: parse_ident(author)?,
            committer: parse_ident(committer)?,
        })
    })
}

/// Parse `git var -l` output whose config part is exactly `config_lines`
/// lines long (see [`var_list_config_lines`]): the next two lines must be
/// `GIT_COMMITTER_IDENT=` and `GIT_AUTHOR_IDENT=`. Anything else → `None`
/// (e.g. config changed between the snapshot and `git var`).
pub fn parse_var_list_anchored(stdout: &str, config_lines: usize) -> Option<EffectiveIdentity> {
    let mut lines = stdout.split('\n').skip(config_lines).map(strip_cr);
    let committer = lines.next()?.strip_prefix(COMMITTER_VAR)?;
    let author = lines.next()?.strip_prefix(AUTHOR_VAR)?;
    Some(EffectiveIdentity {
        author: parse_ident(author)?,
        committer: parse_ident(committer)?,
    })
}

/// How many lines `git var -l` prints for the config entries in `snapshot`
/// (`key=value` per entry, plus one per newline inside a value).
pub fn var_list_config_lines(snapshot: &ConfigSnapshot) -> usize {
    snapshot
        .entries()
        .iter()
        .map(|e| 1 + e.value.as_deref().map_or(0, |v| v.matches('\n').count()))
        .sum()
}

/// git error text (C locale) meaning "no usable identity" rather than a failure.
const UNSET_IDENT_ERRORS: &[&str] = &[
    "auto-detection is disabled",
    "unable to auto-detect email address",
    "empty ident name",
    "Please tell me who you are",
];

/// The identities `git commit` would use in `cwd`, or `Ok(None)` when it would
/// refuse (identity unset, `user.useConfigOnly` without a configured value,
/// an empty configured `user.name`, an empty `GIT_AUTHOR_NAME`, …) — a valid
/// state, not an error.
///
/// Asks git in strict mode, one ident at a time (`git var GIT_AUTHOR_IDENT`,
/// then `git var GIT_COMMITTER_IDENT`): exactly `git commit`'s rules, no
/// parsing of config text. Up to 2 spawns; hooks use [`effective_identity_with`].
pub fn effective_identity(git: &Git, cwd: &Path) -> Result<Option<EffectiveIdentity>> {
    let Some(author) = strict_ident(git, cwd, "GIT_AUTHOR_IDENT")? else {
        return Ok(None);
    };
    let Some(committer) = strict_ident(git, cwd, "GIT_COMMITTER_IDENT")? else {
        return Ok(None);
    };
    Ok(Some(EffectiveIdentity { author, committer }))
}

/// `git var <var>` (strict). `Ok(None)` when git refuses the identity.
fn strict_ident(git: &Git, cwd: &Path, var: &str) -> Result<Option<Identity>> {
    // C locale so the stderr messages we match are not translated.
    let git_c = git.clone().env("LC_ALL", "C");
    let args = ["var", var];
    let out = git_c.output(cwd, &args, None)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if UNSET_IDENT_ERRORS.iter().any(|m| stderr.contains(m)) {
            return Ok(None);
        }
        return Err(crate::gitconfig::git_failed(&args, &out));
    }
    Ok(parse_ident(String::from_utf8_lossy(&out.stdout).trim()))
}

/// Like [`effective_identity`], using a config `snapshot` read from the same
/// `cwd` (one `git var -l` spawn instead of two strict ones).
///
/// `git var -l` computes idents non-strictly (it prints an auto-detected
/// `user@host` or the account's display name where `git commit` refuses), so
/// the strict rules are re-checked from the snapshot: `user.useConfigOnly`
/// without a configured/env name or email, an auto-detected bogus email
/// (`…(none)`), an empty ident name, an empty configured `user.name` or an
/// empty `GIT_<ROLE>_NAME` env var (review #11) → `Ok(None)`.
///
/// If the output doesn't line up with the snapshot (config changed in
/// between), falls back to the strict path (2 more spawns, rare).
pub fn effective_identity_with(
    git: &Git,
    cwd: &Path,
    snapshot: &ConfigSnapshot,
) -> Result<Option<EffectiveIdentity>> {
    // C locale so the stderr messages we match are not translated.
    let git_c = git.clone().env("LC_ALL", "C");
    let args = ["var", "-l"];
    let out = git_c.output(cwd, &args, None)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if UNSET_IDENT_ERRORS.iter().any(|m| stderr.contains(m)) {
            return Ok(None);
        }
        return Err(crate::gitconfig::git_failed(&args, &out));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let Some(ident) = parse_var_list_anchored(&stdout, var_list_config_lines(snapshot)) else {
        return effective_identity(git, cwd);
    };
    let facts = IdentFacts::from_snapshot(snapshot);
    let author_ok = strict_ok(git, &facts, &ident.author, Role::Author);
    let committer_ok = strict_ok(git, &facts, &ident.committer, Role::Committer);
    Ok((author_ok && committer_ok).then_some(ident))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Author,
    Committer,
}

impl Role {
    fn env_prefix(self) -> &'static str {
        match self {
            Role::Author => "GIT_AUTHOR_",
            Role::Committer => "GIT_COMMITTER_",
        }
    }
}

/// The config facts that decide strict ident validity, from the snapshot
/// (never from `git var -l` text, which can't be split reliably).
#[derive(Debug, Default)]
struct IdentFacts {
    use_config_only: bool,
    /// Lower-case keys present among `user|author|committer.name|email`.
    keys: Vec<String>,
    /// Effective values of `user.name`, `author.name`, `committer.name`.
    user_name: Option<String>,
    author_name: Option<String>,
    committer_name: Option<String>,
}

impl IdentFacts {
    fn from_snapshot(s: &ConfigSnapshot) -> Self {
        let mut keys = Vec::new();
        for key in [
            "user.name",
            "user.email",
            "author.name",
            "author.email",
            "committer.name",
            "committer.email",
        ] {
            if s.get(key).is_some() {
                keys.push(key.to_string());
            }
        }
        Self {
            use_config_only: s.get("user.useConfigOnly").is_some_and(parse_bool),
            keys,
            user_name: s.get("user.name").map(str::to_string),
            author_name: s.get("author.name").map(str::to_string),
            committer_name: s.get("committer.name").map(str::to_string),
        }
    }

    fn has(&self, key: &str) -> bool {
        self.keys.iter().any(|k| k == key)
    }

    fn role_name(&self, role: Role) -> Option<&str> {
        match role {
            Role::Author => self.author_name.as_deref(),
            Role::Committer => self.committer_name.as_deref(),
        }
    }
}

fn parse_bool(v: &str) -> bool {
    let v = v.trim();
    ["true", "yes", "on", "1"]
        .iter()
        .any(|t| v.eq_ignore_ascii_case(t))
        || v.parse::<i64>().is_ok_and(|n| n != 0)
}

/// Would `git commit` accept this ident? Mirrors git's `fmt_ident` strict mode.
fn strict_ok(git: &Git, facts: &IdentFacts, id: &Identity, role: Role) -> bool {
    if id.name.is_empty() {
        return false;
    }
    if id.email.ends_with("(none)") {
        // Auto-detected from a host name without a domain: git refuses it.
        return false;
    }
    // Name source, like git: GIT_<ROLE>_NAME (even when empty), else a non-empty
    // <role>.name, else user.name. An empty one is "empty ident name" in strict
    // mode, although `git var -l` shows the account's display name instead.
    let env_name = git.env_var(&format!("{}NAME", role.env_prefix()));
    match env_name {
        Some(v) if v.is_empty() => return false,
        Some(_) => {}
        None => {
            let role_name = facts.role_name(role).filter(|n| !n.is_empty());
            if role_name.is_none() && facts.user_name.as_deref() == Some("") {
                return false;
            }
        }
    }
    if facts.use_config_only {
        // Any of user/author/committer.<field> in config counts as "given" (git
        // keeps one flag for all three); the role's env var counts too.
        let given = |field: &str| {
            git.env_var(&format!(
                "{}{}",
                role.env_prefix(),
                field.to_ascii_uppercase()
            ))
            .is_some()
                || facts.has(&format!("user.{field}"))
                || facts.has(&format!("author.{field}"))
                || facts.has(&format!("committer.{field}"))
        };
        if !given("name") || !given("email") {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitconfig::{ConfigEntry, ConfigScope};

    fn id(name: &str, email: &str) -> Identity {
        Identity {
            name: name.into(),
            email: email.into(),
        }
    }

    fn snap(pairs: &[(&str, Option<&str>)]) -> ConfigSnapshot {
        ConfigSnapshot::from_entries(
            pairs
                .iter()
                .map(|(k, v)| ConfigEntry {
                    scope: ConfigScope::Global,
                    origin: String::new(),
                    key: k.to_string(),
                    value: v.map(str::to_string),
                })
                .collect(),
        )
    }

    #[test]
    fn parse_ident_table() {
        let cases: &[(&str, Option<Identity>)] = &[
            (
                "Jane Doe <jane@x.com> 1730000000 +0100",
                Some(id("Jane Doe", "jane@x.com")),
            ),
            ("Jane Doe <jane@x.com>", Some(id("Jane Doe", "jane@x.com"))),
            (
                "  Jane   <  jane@x.com  >  ",
                Some(id("Jane", "jane@x.com")),
            ),
            (
                "Jörg Ünï 日本 <j@例え.jp> 1 -0000",
                Some(id("Jörg Ünï 日本", "j@例え.jp")),
            ),
            ("<only@email> 1 +0000", Some(id("", "only@email"))),
            ("Name <> 1 +0000", Some(id("Name", ""))),
            ("a <b> c <d>", Some(id("a", "b"))),
            ("Name only", None),
            ("Name <unterminated", None),
            ("", None),
            ("Name > <", None),
        ];
        for (input, want) in cases {
            assert_eq!(&parse_ident(input), want, "input {input:?}");
        }
    }

    #[test]
    fn parse_var_list_first_adjacent_pair() {
        let out = "user.name=X\nalias.evil=!x\nGIT_AUTHOR_IDENT=Fake <fake@x> 1 +0000\n\
                   GIT_COMMITTER_IDENT=Com <c@x> 1730000000 +0000\n\
                   GIT_AUTHOR_IDENT=Auth <a@x> 1730000000 +0000\n\
                   GIT_EDITOR=vim\nGIT_PAGER=less\n";
        let e = parse_var_list(out).unwrap();
        assert_eq!(e.author, id("Auth", "a@x"));
        assert_eq!(e.committer, id("Com", "c@x"));
    }

    /// Review #10, the verified spoof: a multi-line `GIT_PAGER` / `GIT_EDITOR`
    /// is printed after the real idents (real `git var -l` order, Git 2.51).
    #[test]
    fn env_values_after_the_idents_cannot_spoof() {
        let out = "user.name=Repo N\nuser.email=r@x\n\
                   GIT_COMMITTER_IDENT=Repo N <r@x> 1793505365 +0530\n\
                   GIT_AUTHOR_IDENT=Repo N <r@x> 1793505365 +0530\n\
                   GIT_EDITOR=vi\nGIT_COMMITTER_IDENT=Ed <e@x> 1 +0000\n\
                   GIT_SEQUENCE_EDITOR=vi\nGIT_COMMITTER_IDENT=Ed <e@x> 1 +0000\n\
                   GIT_PAGER=less\nGIT_AUTHOR_IDENT=Pager <p@x> 1 +0000\n\
                   GIT_DEFAULT_BRANCH=master\n";
        for e in [
            parse_var_list(out).unwrap(),
            parse_var_list_anchored(out, 2).unwrap(),
        ] {
            assert_eq!(e.author.email, "r@x");
            assert_eq!(e.committer.email, "r@x");
        }
    }

    /// Review #10: a multi-line *config* value that fakes the whole block is
    /// skipped by the snapshot-derived line count.
    #[test]
    fn config_values_cannot_spoof_anchored_parse() {
        let evil = "x\nGIT_COMMITTER_IDENT=Fake <fake@x> 1 +0000\n\
                    GIT_AUTHOR_IDENT=Fake <fake@x> 1 +0000\nGIT_EDITOR=vi";
        let s = snap(&[
            ("user.name", Some("Repo N")),
            ("alias.evil", Some(evil)),
            ("core.flag", None),
            ("user.email", Some("r@x")),
        ]);
        assert_eq!(var_list_config_lines(&s), 7);
        let out = format!(
            "user.name=Repo N\nalias.evil={evil}\ncore.flag\nuser.email=r@x\n\
             GIT_COMMITTER_IDENT=Repo N <r@x> 1 +0000\n\
             GIT_AUTHOR_IDENT=Repo N <r@x> 1 +0000\nGIT_EDITOR=vi\n"
        );
        // The heuristic is fooled…
        assert_eq!(parse_var_list(&out).unwrap().author.email, "fake@x");
        // …the anchored parse is not.
        let e = parse_var_list_anchored(&out, var_list_config_lines(&s)).unwrap();
        assert_eq!(e.author, id("Repo N", "r@x"));
        assert_eq!(e.committer, id("Repo N", "r@x"));
        // Misaligned (config changed in between) → None, never a guess.
        assert_eq!(parse_var_list_anchored(&out, 3), None);
        assert_eq!(parse_var_list_anchored(&out, 99), None);
    }

    #[test]
    fn parse_var_list_crlf_and_missing() {
        let out = "GIT_COMMITTER_IDENT=C <c@x> 1 +0000\r\nGIT_AUTHOR_IDENT=A <a@x> 1 +0000\r\n";
        let e = parse_var_list(out).unwrap();
        assert_eq!(e.author.email, "a@x");
        assert_eq!(
            parse_var_list_anchored(out, 0).unwrap().committer.email,
            "c@x"
        );
        assert_eq!(parse_var_list("GIT_AUTHOR_IDENT=A <a@x> 1 +0000\n"), None);
        assert_eq!(
            parse_var_list("GIT_AUTHOR_IDENT=A\nGIT_COMMITTER_IDENT=C <c>\n"),
            None
        );
        assert_eq!(parse_var_list(""), None);
        assert_eq!(parse_var_list_anchored("", 0), None);
    }

    #[test]
    fn ident_facts_from_snapshot() {
        let c = IdentFacts::from_snapshot(&snap(&[
            ("user.useconfigonly", Some("true")),
            ("user.email", Some("a@x")),
        ]));
        assert!(c.use_config_only);
        assert!(c.has("user.email"));
        assert!(!c.has("user.name"));
        assert!(IdentFacts::from_snapshot(&snap(&[("user.useConfigOnly", None)])).use_config_only);
        assert!(
            !IdentFacts::from_snapshot(&snap(&[("user.useconfigonly", Some("false"))]))
                .use_config_only
        );
        assert!(
            !IdentFacts::from_snapshot(&snap(&[
                ("user.useconfigonly", Some("true")),
                ("user.useconfigonly", Some("no")),
            ]))
            .use_config_only
        );
        // A multi-line value can't smuggle a key in (it was text-parsed before).
        let c = IdentFacts::from_snapshot(&snap(&[("alias.x", Some("a\nuser.email=x"))]));
        assert!(!c.has("user.email"));
    }

    #[test]
    fn strict_rules() {
        let git = Git::with_exe("git")
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL");
        let none = IdentFacts::default();
        assert!(strict_ok(&git, &none, &id("A", "a@x"), Role::Author));
        assert!(!strict_ok(&git, &none, &id("", "a@x"), Role::Author));
        assert!(!strict_ok(
            &git,
            &none,
            &id("A", "me@host.(none)"),
            Role::Author
        ));
        let only = IdentFacts {
            use_config_only: true,
            keys: vec!["user.name".into()],
            ..IdentFacts::default()
        };
        assert!(!strict_ok(
            &git,
            &only,
            &id("A", "a@host.lan"),
            Role::Author
        ));
        let with_env = git.clone().env("GIT_AUTHOR_EMAIL", "a@x");
        assert!(strict_ok(&with_env, &only, &id("A", "a@x"), Role::Author));
        let only_author = IdentFacts {
            use_config_only: true,
            keys: vec!["author.name".into(), "author.email".into()],
            ..IdentFacts::default()
        };
        assert!(strict_ok(
            &git,
            &only_author,
            &id("A", "a@x"),
            Role::Committer
        ));
    }

    /// Review #11: `git var -l` shows the account name for an empty configured
    /// `user.name`, but `git commit` refuses ("empty ident name").
    #[test]
    fn empty_configured_or_env_name_is_unset() {
        let git = Git::with_exe("git")
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_COMMITTER_NAME");
        let shown = id("Account Name", "r@x");
        let empty_user = IdentFacts::from_snapshot(&snap(&[("user.name", Some(""))]));
        assert!(!strict_ok(&git, &empty_user, &shown, Role::Author));
        assert!(!strict_ok(&git, &empty_user, &shown, Role::Committer));
        // A non-empty author.name wins over the empty user.name (author only).
        let with_author = IdentFacts::from_snapshot(&snap(&[
            ("user.name", Some("")),
            ("author.name", Some("A")),
        ]));
        assert!(strict_ok(&git, &with_author, &shown, Role::Author));
        assert!(!strict_ok(&git, &with_author, &shown, Role::Committer));
        // An empty author.name is ignored by git (user.name is used).
        let empty_author = IdentFacts::from_snapshot(&snap(&[
            ("user.name", Some("N")),
            ("author.name", Some("")),
        ]));
        assert!(strict_ok(&git, &empty_author, &shown, Role::Author));
        // The env var wins over config, and an empty one means "empty name".
        let env_empty = git.clone().env("GIT_AUTHOR_NAME", "");
        let ok_cfg = IdentFacts::from_snapshot(&snap(&[("user.name", Some("N"))]));
        assert!(!strict_ok(&env_empty, &ok_cfg, &shown, Role::Author));
        assert!(strict_ok(&env_empty, &ok_cfg, &shown, Role::Committer));
        let env_set = git.clone().env("GIT_AUTHOR_NAME", "E");
        assert!(strict_ok(&env_set, &empty_user, &shown, Role::Author));
    }
}
