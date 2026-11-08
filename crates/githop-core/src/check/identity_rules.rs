//! Which commit emails count as **wrong** — shared by the push check and Fix
//! (Review 2026-11-01 #3, ADR 0003 #4).
//!
//! An email is wrong only when it is **one of the user's own identities** that
//! isn't the owning profile's email. The user's identities are:
//!
//! - every profile's email;
//! - the global/system `user.email` (and `author.email` / `committer.email`);
//! - `user.email` in `backups/gitconfig.original` (the identity before GitHop);
//! - the `EMAIL` env var (git's fallback for `user.email`);
//! - an email git auto-detected on this machine: `<user>@<host>` (also
//!   `<user>@<host>.<domain>`) or anything containing `(none)`.
//!
//! All comparisons are case-insensitive. Any other email belongs to someone else
//! (a merged PR, `git am`, a cherry-pick from a fork): never an offender, and Fix
//! never re-stamps it. The committer `noreply@github.com` is always allowed.
//!
//! Building the rules spawns nothing: the config snapshot is the one the check
//! already read, `gitconfig.original` is a small local file, env comes through
//! [`Git::env_var`].

use std::collections::HashSet;

use crate::backup;
use crate::git::Git;
use crate::gitconfig::{ConfigScope, ConfigSnapshot};
use crate::paths::Paths;
use crate::profile::{Profile, ProfileSet};

use super::push::{LogCommit, OffendingCommit};

/// Committer emails that never count as a mismatch (GitHub web-flow merges pulled locally).
pub const ALLOWED_COMMITTERS: [&str; 1] = ["noreply@github.com"];

/// Config keys whose global/system values are the user's own emails.
const IDENTITY_KEYS: [&str; 3] = ["user.email", "author.email", "committer.email"];

/// Largest `gitconfig.original` read (it is a copy of `~/.gitconfig`).
const MAX_ORIGINAL_BYTES: u64 = 1024 * 1024;

fn norm(email: &str) -> String {
    email.trim().to_lowercase()
}

/// The owning profile's email plus every email that is the user's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRules {
    owner: String,
    user_emails: HashSet<String>,
    /// Login name, lower-case, for the auto-detected `<user>@<host>` form.
    login: Option<String>,
    /// Host name, lower-case.
    host: Option<String>,
}

impl IdentityRules {
    /// Rules from explicit parts (unit tests, callers that already have them).
    pub fn new<'a>(
        owner_email: &str,
        user_emails: impl IntoIterator<Item = &'a str>,
        login: Option<&str>,
        host: Option<&str>,
    ) -> Self {
        let clean = |s: Option<&str>| {
            s.map(norm)
                .filter(|v| !v.is_empty() && !v.contains(['@', ' ']))
        };
        Self {
            owner: norm(owner_email),
            user_emails: user_emails
                .into_iter()
                .map(norm)
                .filter(|e| !e.is_empty())
                .collect(),
            login: clean(login),
            host: clean(host),
        }
    }

    /// Rules for a repo owned by `owner`. No spawn (see the module docs).
    pub fn for_repo(
        git: &Git,
        paths: &Paths,
        set: &ProfileSet,
        owner: &Profile,
        snapshot: &ConfigSnapshot,
    ) -> Self {
        let mut emails: Vec<String> = set.profiles.iter().map(|p| p.email.clone()).collect();
        for e in snapshot.entries() {
            if matches!(e.scope, ConfigScope::Global | ConfigScope::System)
                && IDENTITY_KEYS
                    .iter()
                    .any(|k| crate::gitconfig::key_eq(&e.key, k))
            {
                if let Some(v) = &e.value {
                    emails.push(v.clone());
                }
            }
        }
        emails.extend(original_emails(paths));
        if let Some(v) = git.env_var("EMAIL") {
            emails.push(v.to_string_lossy().into_owned());
        }
        let env = |keys: &[&str]| {
            keys.iter()
                .filter_map(|k| git.env_var(k))
                .map(|v| v.to_string_lossy().into_owned())
                .find(|v| !v.trim().is_empty())
        };
        let login = env(&["USERNAME", "USER", "LOGNAME"]);
        let host = env(&["COMPUTERNAME", "HOSTNAME"]);
        Self::new(
            &owner.email,
            emails.iter().map(String::as_str),
            login.as_deref(),
            host.as_deref(),
        )
    }

    /// The owning profile's email (lower-case).
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// `email` is the owning profile's email.
    pub fn is_owner(&self, email: &str) -> bool {
        norm(email) == self.owner
    }

    /// `email` is one of the user's own identities (owner included).
    pub fn is_user(&self, email: &str) -> bool {
        let e = norm(email);
        if e.is_empty() {
            // git refuses an empty ident unless it auto-detected nothing.
            return true;
        }
        e == self.owner || self.user_emails.contains(&e) || self.is_auto_detected(&e)
    }

    /// git's auto-detected ident of this machine: `(none)` anywhere, or
    /// `<login>@<host>` / `<login>@<host>.<domain>`. `e` is normalised.
    fn is_auto_detected(&self, e: &str) -> bool {
        if e.contains("(none)") {
            return true;
        }
        let (Some(login), Some(host)) = (&self.login, &self.host) else {
            return false;
        };
        let Some((local, domain)) = e.rsplit_once('@') else {
            return false;
        };
        local == login
            && (domain == host
                || domain
                    .strip_prefix(host.as_str())
                    .is_some_and(|rest| rest.starts_with('.')))
    }

    /// An author (or tagger) email is wrong: the user's, but not the owner's.
    pub fn author_wrong(&self, email: &str) -> bool {
        !self.is_owner(email) && self.is_user(email)
    }

    /// A committer email is wrong: like [`Self::author_wrong`], and not an allowed committer.
    pub fn committer_wrong(&self, email: &str) -> bool {
        self.author_wrong(email)
            && !ALLOWED_COMMITTERS
                .iter()
                .any(|a| a.eq_ignore_ascii_case(email.trim()))
    }

    /// Someone else's email (neither the owner's nor any of the user's).
    pub fn is_foreign(&self, email: &str) -> bool {
        !self.is_user(email)
    }
}

/// Commits whose author or committer email is wrong under `rules`. Pure.
pub fn find_offenders(commits: &[LogCommit], rules: &IdentityRules) -> Vec<OffendingCommit> {
    commits
        .iter()
        .filter_map(|c| {
            let author_mismatch = rules.author_wrong(&c.author_email);
            let committer_mismatch = rules.committer_wrong(&c.committer_email);
            (author_mismatch || committer_mismatch).then(|| OffendingCommit {
                sha: c.sha.clone(),
                subject: c.subject.clone(),
                author_email: c.author_email.clone(),
                committer_email: c.committer_email.clone(),
                author_mismatch,
                committer_mismatch,
            })
        })
        .collect()
}

/// `email` values in `[user]`/`[author]`/`[committer]` of `gitconfig.original`.
fn original_emails(paths: &Paths) -> Vec<String> {
    let Some(path) = backup::original_gitconfig(paths) else {
        return Vec::new();
    };
    match std::fs::metadata(&path) {
        Ok(m) if m.len() <= MAX_ORIGINAL_BYTES => {}
        _ => return Vec::new(),
    }
    std::fs::read(&path)
        .map(|b| parse_config_emails(&String::from_utf8_lossy(&b)))
        .unwrap_or_default()
}

/// Minimal git-config reader for `<section>.email` where section is `user`,
/// `author` or `committer` (no subsection). Handles `[user]`, `email = x`,
/// quotes and `#`/`;` comments. Pure.
pub(crate) fn parse_config_emails(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix('[') {
            let header = rest.split(']').next().unwrap_or_default().trim();
            let name = header.split_whitespace().next().unwrap_or_default();
            in_section = !header.contains('"')
                && ["user", "author", "committer"]
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(name));
            // `[user] email = x` on one line.
            let after = rest.split_once(']').map(|(_, a)| a.trim()).unwrap_or("");
            if in_section && !after.is_empty() {
                if let Some(v) = email_value(after) {
                    out.push(v);
                }
            }
            continue;
        }
        if in_section {
            if let Some(v) = email_value(line) {
                out.push(v);
            }
        }
    }
    out
}

fn email_value(line: &str) -> Option<String> {
    let (key, value) = line.split_once('=')?;
    if !key.trim().eq_ignore_ascii_case("email") {
        return None;
    }
    let mut v = String::new();
    let mut quoted = false;
    for c in value.trim().chars() {
        match c {
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => break,
            _ => v.push(c),
        }
    }
    let v = v.trim().to_string();
    (!v.is_empty()).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> IdentityRules {
        IdentityRules::new(
            "Me@Office.example",
            [
                "me@office.example",
                "me@personal.example",
                "old@home.example",
            ],
            Some("Jane"),
            Some("DESKTOP-7"),
        )
    }

    fn lc(ae: &str, ce: &str) -> LogCommit {
        LogCommit {
            sha: "a".repeat(40),
            author_email: ae.into(),
            committer_email: ce.into(),
            subject: "s".into(),
        }
    }

    #[test]
    fn owner_is_never_wrong_case_insensitive() {
        let r = rules();
        assert!(!r.author_wrong("ME@office.example "));
        assert!(r.is_owner("me@OFFICE.example"));
    }

    #[test]
    fn users_other_emails_are_wrong() {
        let r = rules();
        assert!(r.author_wrong("me@personal.example"));
        assert!(r.author_wrong("OLD@home.example"));
        assert!(r.committer_wrong("me@personal.example"));
    }

    #[test]
    fn foreign_emails_are_never_wrong() {
        let r = rules();
        assert!(!r.author_wrong("alice@contrib.example"));
        assert!(!r.committer_wrong("alice@contrib.example"));
        assert!(r.is_foreign("alice@contrib.example"));
        assert!(
            find_offenders(&[lc("alice@contrib.example", "alice@contrib.example")], &r).is_empty()
        );
    }

    #[test]
    fn auto_detected_idents_are_the_users() {
        let r = rules();
        assert!(r.author_wrong("jane@desktop-7"));
        assert!(r.author_wrong("Jane@DESKTOP-7.corp.example"));
        assert!(r.author_wrong("jane@desktop-7.(none)"));
        assert!(r.author_wrong("someone@(none)"));
        assert!(!r.author_wrong("jane@desktop-77"));
        assert!(!r.author_wrong("other@desktop-7"));
        // Without a login/host only `(none)` counts.
        let bare = IdentityRules::new("me@office.example", [], None, None);
        assert!(!bare.author_wrong("jane@desktop-7"));
        assert!(bare.author_wrong("x@y.(none)"));
    }

    #[test]
    fn noreply_committer_allowed_but_not_as_author() {
        let r = IdentityRules::new("me@office.example", ["noreply@github.com"], None, None);
        assert!(!r.committer_wrong("noreply@github.com"));
        assert!(r.author_wrong("noreply@github.com"));
    }

    #[test]
    fn committer_only_mismatch_with_foreign_author() {
        let r = rules();
        let o = find_offenders(&[lc("alice@contrib.example", "me@personal.example")], &r);
        assert_eq!(o.len(), 1);
        assert!(!o[0].author_mismatch && o[0].committer_mismatch);
        assert_eq!(o[0].wrong_email(), "me@personal.example");
    }

    #[test]
    fn config_email_parsing() {
        let text = "[core]\n\temail = not-me@x\n[user]\n\tname = H\n\temail = \"Me@Home.example\" # c\n\
                    [user \"sub\"]\n\temail = sub@x\n[Author] email = a@x\n[committer]\nEmail=c@x;x\n";
        assert_eq!(
            parse_config_emails(text),
            vec!["Me@Home.example", "a@x", "c@x"]
        );
        assert!(parse_config_emails("").is_empty());
    }

    #[test]
    fn for_repo_reads_global_config_original_and_env() {
        use crate::gitconfig::ConfigEntry;
        use crate::profile::ProfileColor;
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path());
        std::fs::create_dir_all(paths.backups_dir()).unwrap();
        std::fs::write(
            paths.backups_dir().join(backup::ORIGINAL_GITCONFIG),
            "[user]\n\temail = before@githop.example\n",
        )
        .unwrap();
        let p = |id: &str, email: &str| Profile {
            id: id.into(),
            label: id.into(),
            name: id.into(),
            email: email.into(),
            color: ProfileColor::Blue,
            ssh_key: std::path::PathBuf::from("k"),
            owners: vec![],
            folders: vec![],
            sign_commits: false,
        };
        let set = ProfileSet {
            version: 1,
            default: None,
            profiles: vec![
                p("office", "me@office.example"),
                p("personal", "me@personal.example"),
            ],
        };
        let entry = |scope, key: &str, v: &str| ConfigEntry {
            scope,
            origin: String::new(),
            key: key.into(),
            value: Some(v.into()),
        };
        let snap = ConfigSnapshot::from_entries(vec![
            entry(ConfigScope::Global, "user.email", "global@me.example"),
            entry(ConfigScope::Local, "user.email", "me@office.example"),
            entry(ConfigScope::Command, "user.email", "cmd@x.example"),
        ]);
        let git = Git::with_exe("git")
            .env("EMAIL", "env@me.example")
            .env("USERNAME", "jane")
            .env("COMPUTERNAME", "BOX");
        let r = IdentityRules::for_repo(&git, &paths, &set, &set.profiles[0], &snap);
        for e in [
            "me@personal.example",
            "global@me.example",
            "before@githop.example",
            "env@me.example",
            "jane@box",
        ] {
            assert!(r.author_wrong(e), "{e}");
        }
        assert!(!r.author_wrong("me@office.example"));
        assert!(!r.author_wrong("cmd@x.example"));
        assert!(!r.author_wrong("alice@contrib.example"));
    }
}
