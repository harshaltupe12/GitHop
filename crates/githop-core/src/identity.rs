//! Effective author/committer identity via `git var` (Plan §8 C8, C9).
//!
//! CONTRACT FILE: types fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::Git;

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

/// Parse `GIT_AUTHOR_IDENT=` / `GIT_COMMITTER_IDENT=` lines out of `git var -l` output.
///
/// `git var -l` prints the config list first and the logical variables last, so
/// the **last** matching line wins (a multi-line config value can't spoof it).
/// `None` unless both idents are present and parse.
pub fn parse_var_list(stdout: &str) -> Option<EffectiveIdentity> {
    let mut author = None;
    let mut committer = None;
    for line in stdout.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(v) = line.strip_prefix(AUTHOR_VAR) {
            author = Some(v);
        } else if let Some(v) = line.strip_prefix(COMMITTER_VAR) {
            committer = Some(v);
        }
    }
    Some(EffectiveIdentity {
        author: parse_ident(author?)?,
        committer: parse_ident(committer?)?,
    })
}

/// git error text (C locale) meaning "no usable identity" rather than a failure.
const UNSET_IDENT_ERRORS: &[&str] = &[
    "auto-detection is disabled",
    "unable to auto-detect email address",
    "empty ident name",
    "Please tell me who you are",
];

/// One spawn (`git var -l`). `Ok(None)` when identity is unset (e.g.
/// `user.useConfigOnly` with no email) — a valid state, not an error.
///
/// `git var -l` computes idents non-strictly (it would print an auto-detected
/// `user@host` even where `git commit` refuses), so the strict rules `git commit`
/// applies are re-checked here from the config lines in the same output:
/// `user.useConfigOnly` without a configured/env name or email, an
/// auto-detected bogus email (`…(none)`), or an empty name → `Ok(None)`.
pub fn effective_identity(git: &Git, cwd: &Path) -> Result<Option<EffectiveIdentity>> {
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
    let Some(ident) = parse_var_list(&stdout) else {
        return Ok(None);
    };
    let cfg = VarConfig::parse(&stdout);
    let author_ok = strict_ok(git, &cfg, &ident.author, "AUTHOR");
    let committer_ok = strict_ok(git, &cfg, &ident.committer, "COMMITTER");
    Ok((author_ok && committer_ok).then_some(ident))
}

/// The few config facts from `git var -l` that decide strict ident validity.
#[derive(Debug, Default)]
struct VarConfig {
    use_config_only: bool,
    /// Lower-case keys seen among `user|author|committer.name|email`.
    keys: Vec<String>,
}

impl VarConfig {
    fn parse(stdout: &str) -> Self {
        let mut cfg = Self::default();
        for line in stdout.lines() {
            if line.starts_with("GIT_") {
                continue;
            }
            let (key, value) = match line.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (line, None),
            };
            let key = key.to_ascii_lowercase();
            match key.as_str() {
                "user.useconfigonly" => cfg.use_config_only = value.is_none_or(parse_bool),
                "user.name" | "user.email" | "author.name" | "author.email" | "committer.name"
                | "committer.email" => cfg.keys.push(key),
                _ => {}
            }
        }
        cfg
    }

    fn has(&self, key: &str) -> bool {
        self.keys.iter().any(|k| k == key)
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
fn strict_ok(git: &Git, cfg: &VarConfig, id: &Identity, env_role: &str) -> bool {
    if id.name.is_empty() {
        return false;
    }
    if id.email.ends_with("(none)") {
        // Auto-detected from a host name without a domain: git refuses it.
        return false;
    }
    if cfg.use_config_only {
        // Any of user/author/committer.<field> in config counts as "given" (git
        // keeps one flag for all three); the role's env var counts too.
        let given = |field: &str| {
            git.env_var(&format!("GIT_{env_role}_{}", field.to_ascii_uppercase()))
                .is_some()
                || cfg.has(&format!("user.{field}"))
                || cfg.has(&format!("author.{field}"))
                || cfg.has(&format!("committer.{field}"))
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

    fn id(name: &str, email: &str) -> Identity {
        Identity {
            name: name.into(),
            email: email.into(),
        }
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
    fn parse_var_list_uses_last_lines() {
        let out = "user.name=X\nalias.evil=!x\nGIT_AUTHOR_IDENT=Fake <fake@x> 1 +0000\n\
                   GIT_COMMITTER_IDENT=Com <c@x> 1730000000 +0000\n\
                   GIT_AUTHOR_IDENT=Auth <a@x> 1730000000 +0000\n\
                   GIT_EDITOR=vim\nGIT_PAGER=less\n";
        let e = parse_var_list(out).unwrap();
        assert_eq!(e.author, id("Auth", "a@x"));
        assert_eq!(e.committer, id("Com", "c@x"));
    }

    #[test]
    fn parse_var_list_crlf_and_missing() {
        let out = "GIT_COMMITTER_IDENT=C <c@x> 1 +0000\r\nGIT_AUTHOR_IDENT=A <a@x> 1 +0000\r\n";
        let e = parse_var_list(out).unwrap();
        assert_eq!(e.author.email, "a@x");
        assert_eq!(parse_var_list("GIT_AUTHOR_IDENT=A <a@x> 1 +0000\n"), None);
        assert_eq!(
            parse_var_list("GIT_AUTHOR_IDENT=A\nGIT_COMMITTER_IDENT=C <c>\n"),
            None
        );
        assert_eq!(parse_var_list(""), None);
    }

    #[test]
    fn var_config_flags() {
        let c = VarConfig::parse("user.useconfigonly=true\nuser.email=a@x\nGIT_EDITOR=x\n");
        assert!(c.use_config_only);
        assert!(c.has("user.email"));
        assert!(!c.has("user.name"));
        assert!(VarConfig::parse("user.useconfigonly\n").use_config_only);
        assert!(!VarConfig::parse("user.useconfigonly=false\n").use_config_only);
        assert!(
            !VarConfig::parse("user.useconfigonly=true\nuser.useconfigonly=no\n").use_config_only
        );
    }

    #[test]
    fn strict_rules() {
        let git = Git::with_exe("git")
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL");
        let none = VarConfig::default();
        assert!(strict_ok(&git, &none, &id("A", "a@x"), "AUTHOR"));
        assert!(!strict_ok(&git, &none, &id("", "a@x"), "AUTHOR"));
        assert!(!strict_ok(
            &git,
            &none,
            &id("A", "me@host.(none)"),
            "AUTHOR"
        ));
        let only = VarConfig {
            use_config_only: true,
            keys: vec!["user.name".into()],
        };
        assert!(!strict_ok(&git, &only, &id("A", "a@host.lan"), "AUTHOR"));
        let with_env = git.clone().env("GIT_AUTHOR_EMAIL", "a@x");
        assert!(strict_ok(&with_env, &only, &id("A", "a@x"), "AUTHOR"));
        let only_author = VarConfig {
            use_config_only: true,
            keys: vec!["author.name".into(), "author.email".into()],
        };
        assert!(strict_ok(&git, &only_author, &id("A", "a@x"), "COMMITTER"));
    }
}
