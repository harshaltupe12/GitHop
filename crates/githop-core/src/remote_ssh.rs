//! GitHub HTTPS → SSH remote switching (ADR 0006).
//!
//! A pin sets `core.sshCommand`, which only SSH remotes use. Over
//! `https://github.com/…` git signs in with the account Windows saved (Git
//! Credential Manager), so a repo pinned to Personal can still push as the
//! Office account (403). Switching the remote to `git@github.com:owner/repo.git`
//! makes the profile's key do the push.
//!
//! - Only GitHub HTTPS remotes are touched; any other URL is left as is.
//! - Writes go through `git remote set-url` (argument arrays), so both the
//!   fetch URL and any GitHub HTTPS push URL change.
//! - The old URL is kept in local config (`githop.saved-remote-url-<name>`,
//!   `githop.saved-remote-pushurl-<name>`) for [`restore_remote_url`]. A
//!   password or token in the URL is never copied: the saved value keeps the
//!   user name only (CLAUDE.md rule 8). Switching removes the token from the
//!   repo's config; undo puts back the URL without it.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::gitconfig::{self, ConfigScope, ConfigSnapshot};
use crate::repo::Repo;
use crate::settings::RemoteHttpsPolicy;

/// Local key prefix holding a remote's URL from before the switch.
pub const SAVED_URL_PREFIX: &str = "githop.saved-remote-url-";
/// Local key prefix holding a remote's push URL(s) from before the switch.
pub const SAVED_PUSHURL_PREFIX: &str = "githop.saved-remote-pushurl-";

/// A GitHub HTTPS remote that a switch would change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpsRemote {
    /// Remote name, e.g. `origin`.
    pub name: String,
    /// The HTTPS URL as configured, without any password or token.
    pub url: String,
    /// What the remote becomes: `git@github.com:owner/repo.git`.
    pub ssh_url: String,
}

/// What [`apply_policy`] did (or the caller should do) after a pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpsAction {
    /// No GitHub HTTPS remote.
    Nothing,
    /// Policy `auto`: these remotes were switched.
    Switched(Vec<HttpsRemote>),
    /// Policy `ask`: ask the user, then call [`switch_all_to_ssh`].
    Ask(Vec<HttpsRemote>),
    /// Policy `never`: leave them; a warning is enough.
    Warn(Vec<HttpsRemote>),
}

/// `(owner, repo)` of `https://[user[:pw]@](www.)github.com[:443]/owner/repo[.git][/]`
/// (also `http://`), keeping the case as written. Anything else → `None`. Pure.
pub fn github_https_parts(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let (authority, path) = rest.split_once('/')?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host_port.split_once(':') {
        Some((h, port)) if port == "443" || port == "80" => h,
        Some(_) => return None,
        None => host_port,
    };
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let [owner, repo] = segments.as_slice() else {
        return None;
    };
    let repo = repo
        .strip_suffix(".git")
        .or_else(|| repo.strip_suffix(".GIT"))
        .unwrap_or(repo);
    let ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && !s.starts_with('-')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
    };
    (ok(owner) && ok(repo)).then(|| (owner.to_string(), repo.to_string()))
}

/// True for a GitHub HTTPS remote URL. Pure.
pub fn is_github_https(url: &str) -> bool {
    github_https_parts(url).is_some()
}

/// `git@github.com:<owner>/<repo>.git`.
pub fn ssh_url_for(owner: &str, repo: &str) -> String {
    format!("git@github.com:{owner}/{repo}.git")
}

/// SSH form of a GitHub HTTPS URL; `None` for any other URL. Pure.
pub fn ssh_equivalent(url: &str) -> Option<String> {
    github_https_parts(url).map(|(o, r)| ssh_url_for(&o, &r))
}

/// `https://user:token@host/…` → `https://user@host/…` (no secret is ever copied). Pure.
pub fn strip_password(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (rest, None),
    };
    let authority = match authority.rsplit_once('@') {
        Some((userinfo, host)) => {
            let user = userinfo.split_once(':').map_or(userinfo, |(u, _)| u);
            if user.is_empty() {
                host.to_string()
            } else {
                format!("{user}@{host}")
            }
        }
        None => authority.to_string(),
    };
    match path {
        Some(p) => format!("{scheme}://{authority}/{p}"),
        None => format!("{scheme}://{authority}"),
    }
}

/// Config variable suffix for a remote name: ASCII letters and digits stay,
/// every other byte becomes `-xx` (hex), so any remote name gives a valid,
/// collision-free key. Pure.
pub fn key_suffix(remote: &str) -> String {
    let mut out = String::with_capacity(remote.len());
    for b in remote.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("-{b:02x}"));
        }
    }
    out
}

fn saved_url_key(remote: &str) -> String {
    format!("{SAVED_URL_PREFIX}{}", key_suffix(remote))
}

fn saved_pushurl_key(remote: &str) -> String {
    format!("{SAVED_PUSHURL_PREFIX}{}", key_suffix(remote))
}

/// A name git and GitHop can pass as an argument (never read as an option).
fn valid_remote_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && !name.chars().any(char::is_control)
}

/// Repo-local `remote.<name>.<var>` values, in file order.
fn local_values<'a>(snapshot: &'a ConfigSnapshot, name: &str, var: &str) -> Vec<&'a str> {
    let key = format!("remote.{name}.{var}");
    snapshot
        .entries()
        .iter()
        .filter(|e| {
            matches!(e.scope, ConfigScope::Local | ConfigScope::Unknown)
                && gitconfig::key_eq(&e.key, &key)
        })
        .filter_map(|e| e.value.as_deref())
        .collect()
}

/// Remote names defined in the repo's own config.
fn local_remotes(snapshot: &ConfigSnapshot) -> Vec<String> {
    snapshot
        .remotes()
        .into_iter()
        .filter(|n| !local_values(snapshot, n, "url").is_empty())
        .collect()
}

/// Does `url` stay HTTPS after `url.<base>.insteadOf` rewriting? (A user who
/// already rewrites `https://github.com/` to SSH has nothing to switch.)
fn effective_https(snapshot: &ConfigSnapshot, url: &str) -> bool {
    match snapshot.rewrite_url(url) {
        Some(rewritten) => is_github_https(&rewritten),
        None => true,
    }
}

/// GitHub HTTPS remotes of the repo (fetch URL or push URL), one entry per
/// remote. Pure (reads the snapshot; only repo-local remotes count, since only
/// those `git remote set-url` changes).
pub fn https_remotes(snapshot: &ConfigSnapshot) -> Vec<HttpsRemote> {
    let mut out = Vec::new();
    for name in local_remotes(snapshot) {
        if !valid_remote_name(&name) {
            continue;
        }
        let url = local_values(snapshot, &name, "url")
            .first()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let push = local_values(snapshot, &name, "pushurl");
        let https_url = [url.as_str()]
            .into_iter()
            .chain(push.iter().copied())
            .find(|u| is_github_https(u) && effective_https(snapshot, u));
        if let Some(u) = https_url {
            if let Some(ssh_url) = ssh_equivalent(u) {
                out.push(HttpsRemote {
                    name: name.clone(),
                    url: strip_password(u),
                    ssh_url,
                });
            }
        }
    }
    out
}

/// Remotes GitHop switched to SSH that can still be restored. Pure.
pub fn switched_remotes(snapshot: &ConfigSnapshot) -> Vec<String> {
    let has = |key: &str| {
        snapshot
            .entries()
            .iter()
            .any(|e| e.scope != ConfigScope::Global && e.key.eq_ignore_ascii_case(key))
    };
    snapshot
        .remotes()
        .into_iter()
        .filter(|n| has(&saved_url_key(n)) || has(&saved_pushurl_key(n)))
        .collect()
}

/// What to do with `remotes` under `policy`. Pure.
pub fn decide(policy: RemoteHttpsPolicy, remotes: Vec<HttpsRemote>) -> HttpsAction {
    if remotes.is_empty() {
        return HttpsAction::Nothing;
    }
    match policy {
        RemoteHttpsPolicy::Auto => HttpsAction::Switched(remotes),
        RemoteHttpsPolicy::Ask => HttpsAction::Ask(remotes),
        RemoteHttpsPolicy::Never => HttpsAction::Warn(remotes),
    }
}

/// `git remote set-url [--push] <name> <new> <regex for old>`.
fn set_url(repo: &Repo, name: &str, push: bool, new: &str, old: &str) -> Result<()> {
    let pattern = format!("^{}$", gitconfig::regex_literal(old));
    let mut args = vec!["remote", "set-url"];
    if push {
        args.push("--push");
    }
    args.extend([name, new, pattern.as_str()]);
    repo.git().run(repo.root(), &args).map(drop)
}

/// Switch remote `name` to SSH: its fetch URL and every GitHub HTTPS push URL.
/// The old values are saved first (password removed). `Ok(None)` when the
/// remote isn't a GitHub HTTPS remote (nothing changed).
pub fn switch_remote_to_ssh(repo: &Repo, name: &str) -> Result<Option<HttpsRemote>> {
    let snapshot = repo.config_snapshot()?;
    switch_with(repo, &snapshot, name)
}

fn switch_with(repo: &Repo, snapshot: &ConfigSnapshot, name: &str) -> Result<Option<HttpsRemote>> {
    if !valid_remote_name(name) {
        return Err(CoreError::GitFailed {
            args: "remote set-url".into(),
            code: None,
            stderr: format!("GitHop can't switch a remote called \"{name}\""),
        });
    }
    let Some(found) = https_remotes(snapshot).into_iter().find(|r| r.name == name) else {
        return Ok(None);
    };
    let git = repo.git();
    let root = repo.root();
    let urls = local_values(snapshot, name, "url");
    let pushes: Vec<String> = local_values(snapshot, name, "pushurl")
        .into_iter()
        .filter(|u| is_github_https(u) && effective_https(snapshot, u))
        .map(str::to_string)
        .collect();

    // Save first, so an interrupted switch can always be undone.
    let fetch = urls
        .first()
        .filter(|u| is_github_https(u) && effective_https(snapshot, u))
        .map(|u| u.to_string());
    if let Some(u) = &fetch {
        gitconfig::set_local(git, root, &saved_url_key(name), &strip_password(u))?;
    }
    if !pushes.is_empty() {
        gitconfig::unset_local(git, root, &saved_pushurl_key(name))?;
        for p in &pushes {
            git.run(
                root,
                &[
                    "config",
                    "--local",
                    "--add",
                    &saved_pushurl_key(name),
                    &strip_password(p),
                ],
            )?;
        }
    }
    if let Some(u) = &fetch {
        if let Some(ssh) = ssh_equivalent(u) {
            set_url(repo, name, false, &ssh, u)?;
        }
    }
    for p in &pushes {
        if let Some(ssh) = ssh_equivalent(p) {
            set_url(repo, name, true, &ssh, p)?;
        }
    }
    Ok(Some(found))
}

/// Switch every GitHub HTTPS remote of the repo to SSH. Returns what changed.
pub fn switch_all_to_ssh(repo: &Repo) -> Result<Vec<HttpsRemote>> {
    let snapshot = repo.config_snapshot()?;
    let mut out = Vec::new();
    for r in https_remotes(&snapshot) {
        if let Some(done) = switch_with(repo, &snapshot, &r.name)? {
            out.push(done);
        }
    }
    Ok(out)
}

/// Put back the URL(s) GitHop saved for remote `name`. A URL is restored only
/// while the remote still has the SSH URL GitHop set (a URL the user changed
/// since is left alone). The saved copies are removed either way. Returns
/// whether anything was restored.
pub fn restore_remote_url(repo: &Repo, name: &str) -> Result<bool> {
    let snapshot = repo.config_snapshot()?;
    restore_with(repo, &snapshot, name)
}

fn saved_values<'a>(snapshot: &'a ConfigSnapshot, key: &str) -> Vec<&'a str> {
    snapshot
        .entries()
        .iter()
        .filter(|e| e.key.eq_ignore_ascii_case(key))
        .filter_map(|e| e.value.as_deref())
        .collect()
}

fn restore_with(repo: &Repo, snapshot: &ConfigSnapshot, name: &str) -> Result<bool> {
    if !valid_remote_name(name) {
        return Ok(false);
    }
    let git = repo.git();
    let root = repo.root();
    let mut restored = false;
    let url_key = saved_url_key(name);
    if let Some(saved) = saved_values(snapshot, &url_key).last().copied() {
        let current = local_values(snapshot, name, "url");
        if let (Some(cur), Some(ssh)) = (current.first(), ssh_equivalent(saved)) {
            if *cur == ssh {
                set_url(repo, name, false, saved, cur)?;
                restored = true;
            }
        }
        gitconfig::unset_local(git, root, &url_key)?;
    }
    let push_key = saved_pushurl_key(name);
    let saved_pushes = saved_values(snapshot, &push_key);
    if !saved_pushes.is_empty() {
        let current = local_values(snapshot, name, "pushurl");
        for saved in saved_pushes {
            if let Some(ssh) = ssh_equivalent(saved) {
                if current.contains(&ssh.as_str()) {
                    set_url(repo, name, true, saved, &ssh)?;
                    restored = true;
                }
            }
        }
        gitconfig::unset_local(git, root, &push_key)?;
    }
    Ok(restored)
}

/// [`restore_remote_url`] for every remote GitHop switched. Returns the
/// remotes restored.
pub fn restore_all(repo: &Repo) -> Result<Vec<String>> {
    let snapshot = repo.config_snapshot()?;
    let mut out = Vec::new();
    for name in switched_remotes(&snapshot) {
        if restore_with(repo, &snapshot, &name)? {
            out.push(name);
        }
    }
    Ok(out)
}

/// After a pin: apply `policy` to the repo's GitHub HTTPS remotes (`auto`
/// switches them; `ask` / `never` change nothing and report them).
pub fn apply_policy(repo: &Repo, policy: RemoteHttpsPolicy) -> Result<HttpsAction> {
    let snapshot = repo.config_snapshot()?;
    apply_policy_with(repo, &snapshot, policy)
}

/// [`apply_policy`] with a snapshot already read from this repo (no read
/// spawn; hooks use this). Only the remotes matter, so a snapshot read before
/// a pin is fine.
pub fn apply_policy_with(
    repo: &Repo,
    snapshot: &ConfigSnapshot,
    policy: RemoteHttpsPolicy,
) -> Result<HttpsAction> {
    let action = decide(policy, https_remotes(snapshot));
    if let HttpsAction::Switched(list) = &action {
        let mut done = Vec::new();
        for r in list {
            if let Some(d) = switch_with(repo, snapshot, &r.name)? {
                done.push(d);
            }
        }
        return Ok(HttpsAction::Switched(done));
    }
    Ok(action)
}

/// "origin" / "origin and upstream" / "a, b and c".
pub fn names_and(remotes: &[HttpsRemote]) -> String {
    let names: Vec<&str> = remotes.iter().map(|r| r.name.as_str()).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The one-line problem statement for a repo whose pushes use HTTPS.
pub fn https_problem(profile_label: &str) -> String {
    format!(
        "Uses HTTPS: {profile_label}'s SSH key isn't used. Windows signs in with the account it saved."
    )
}

/// Hook / CLI line after a pin, for what [`apply_policy`] did (`None` = nothing to say).
pub fn pin_message(action: &HttpsAction, profile_label: &str) -> Option<crate::check::Message> {
    use crate::check::Message;
    match action {
        HttpsAction::Nothing => None,
        HttpsAction::Switched(list) if list.is_empty() => None,
        HttpsAction::Switched(list) => Some(
            Message::ok(format!(
                "GitHop: switched {} to SSH, so pushes use {profile_label}'s key",
                names_and(list)
            ))
            .hint("Undo: githop remote undo"),
        ),
        HttpsAction::Ask(_) | HttpsAction::Warn(_) => Some(
            Message::warning(format!("GitHop: {}", https_problem(profile_label)))
                .hint("Fix: githop remote ssh"),
        ),
    }
}

/// Pre-push warning (never blocks): the push goes to a GitHub HTTPS URL (`$2`
/// of pre-push, after git's URL rewriting) while the repo has a profile. Off
/// under policy `never`. Pure.
pub fn https_push_warning(
    profile_label: &str,
    remote_url: Option<&str>,
    policy: RemoteHttpsPolicy,
) -> Option<crate::check::Message> {
    if policy == RemoteHttpsPolicy::Never || !remote_url.is_some_and(is_github_https) {
        return None;
    }
    Some(
        crate::check::Message::warning(format!(
            "GitHop: this push uses HTTPS, so Windows signs in with the account it saved — not {profile_label}'s key."
        ))
        .hint("Fix: githop remote ssh (or the Repos screen)"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitconfig::ConfigEntry;

    fn snap(entries: &[(&str, &str)]) -> ConfigSnapshot {
        ConfigSnapshot::from_entries(
            entries
                .iter()
                .map(|(k, v)| ConfigEntry {
                    scope: ConfigScope::Local,
                    origin: "file:.git/config".into(),
                    key: (*k).into(),
                    value: Some((*v).into()),
                })
                .collect(),
        )
    }

    #[test]
    fn github_https_table() {
        let gh = |o: &str, r: &str| Some((o.to_string(), r.to_string()));
        let cases: &[(&str, Option<(String, String)>)] = &[
            (
                "https://github.com/harshaltupe12/GitHop",
                gh("harshaltupe12", "GitHop"),
            ),
            (
                "https://github.com/harshaltupe12/GitHop.git",
                gh("harshaltupe12", "GitHop"),
            ),
            ("https://github.com/o/r.git/", gh("o", "r")),
            ("https://user@github.com/o/r.git", gh("o", "r")),
            ("https://user:ghp_secret@github.com/o/r.git", gh("o", "r")),
            (
                "HTTPS://GitHub.com/Acme/My.Repo.js",
                gh("Acme", "My.Repo.js"),
            ),
            ("https://www.github.com/o/r", gh("o", "r")),
            ("https://github.com:443/o/r", gh("o", "r")),
            ("http://github.com/o/r", gh("o", "r")),
            ("https://github.com/o/r?tab=x", gh("o", "r")),
            ("https://github.com/o", None),
            ("https://github.com/o/r/tree/main", None),
            ("https://github.com:8443/o/r", None),
            ("https://gitlab.com/o/r.git", None),
            ("https://github.example.com/o/r.git", None),
            ("https://notgithub.com/o/r", None),
            ("git@github.com:o/r.git", None),
            ("ssh://git@github.com/o/r.git", None),
            ("file:///C:/x/r.git", None),
            ("https://github.com/-o/r", None),
            ("https://github.com/o/r name", None),
            ("", None),
        ];
        for (url, want) in cases {
            assert_eq!(&github_https_parts(url), want, "url {url:?}");
        }
        assert_eq!(
            ssh_equivalent("https://github.com/harshaltupe12/GitHop").as_deref(),
            Some("git@github.com:harshaltupe12/GitHop.git")
        );
        assert_eq!(ssh_equivalent("https://gitlab.com/o/r"), None);
    }

    #[test]
    fn passwords_are_never_kept() {
        assert_eq!(
            strip_password("https://user:ghp_secret@github.com/o/r.git"),
            "https://user@github.com/o/r.git"
        );
        assert_eq!(
            strip_password("https://:tok@github.com/o/r"),
            "https://github.com/o/r"
        );
        assert_eq!(
            strip_password("https://github.com/o/r"),
            "https://github.com/o/r"
        );
        assert_eq!(
            strip_password("https://u@github.com"),
            "https://u@github.com"
        );
    }

    #[test]
    fn key_suffix_is_valid_and_distinct() {
        assert_eq!(key_suffix("origin"), "origin");
        assert_eq!(key_suffix("my-fork"), "my-2dfork");
        assert_eq!(key_suffix("a_b.c"), "a-5fb-2ec");
        assert_ne!(key_suffix("a-b"), key_suffix("a-2db"));
    }

    #[test]
    fn lists_only_github_https_remotes() {
        let s = snap(&[
            (
                "remote.origin.url",
                "https://user:tok@github.com/me/app.git",
            ),
            ("remote.upstream.url", "git@github.com:org/app.git"),
            ("remote.lab.url", "https://gitlab.com/me/app.git"),
            ("remote.mirror.url", "git@github.com:me/app.git"),
            ("remote.mirror.pushurl", "https://github.com/me/app"),
        ]);
        let got = https_remotes(&s);
        assert_eq!(
            got,
            vec![
                HttpsRemote {
                    name: "mirror".into(),
                    url: "https://github.com/me/app".into(),
                    ssh_url: "git@github.com:me/app.git".into(),
                },
                HttpsRemote {
                    name: "origin".into(),
                    url: "https://user@github.com/me/app.git".into(),
                    ssh_url: "git@github.com:me/app.git".into(),
                },
            ]
        );
    }

    #[test]
    fn instead_of_ssh_rewrite_means_nothing_to_switch() {
        let s = snap(&[
            ("remote.origin.url", "https://github.com/me/app.git"),
            ("url.git@github.com:.insteadof", "https://github.com/"),
        ]);
        assert!(https_remotes(&s).is_empty());
    }

    #[test]
    fn decide_follows_policy() {
        let r = vec![HttpsRemote {
            name: "origin".into(),
            url: "https://github.com/o/r".into(),
            ssh_url: "git@github.com:o/r.git".into(),
        }];
        assert_eq!(
            decide(RemoteHttpsPolicy::Auto, vec![]),
            HttpsAction::Nothing
        );
        assert_eq!(
            decide(RemoteHttpsPolicy::Auto, r.clone()),
            HttpsAction::Switched(r.clone())
        );
        assert_eq!(
            decide(RemoteHttpsPolicy::Ask, r.clone()),
            HttpsAction::Ask(r.clone())
        );
        assert_eq!(
            decide(RemoteHttpsPolicy::Never, r.clone()),
            HttpsAction::Warn(r)
        );
    }

    #[test]
    fn push_warning_only_for_github_https_and_not_under_never() {
        let w = https_push_warning(
            "Personal",
            Some("https://github.com/harshaltupe12/GitHop.git"),
            RemoteHttpsPolicy::Ask,
        )
        .unwrap();
        assert_eq!(w.level, crate::check::Level::Warning);
        assert_eq!(
            w.text,
            "GitHop: this push uses HTTPS, so Windows signs in with the account it saved — not Personal's key."
        );
        assert_eq!(
            w.hint.as_deref(),
            Some("Fix: githop remote ssh (or the Repos screen)")
        );
        assert!(
            https_push_warning("P", Some("https://github.com/o/r"), RemoteHttpsPolicy::Auto)
                .is_some()
        );
        assert!(https_push_warning(
            "P",
            Some("https://github.com/o/r"),
            RemoteHttpsPolicy::Never
        )
        .is_none());
        assert!(
            https_push_warning("P", Some("git@github.com:o/r.git"), RemoteHttpsPolicy::Ask)
                .is_none()
        );
        assert!(
            https_push_warning("P", Some("https://gitlab.com/o/r"), RemoteHttpsPolicy::Ask)
                .is_none()
        );
        assert!(https_push_warning("P", None, RemoteHttpsPolicy::Ask).is_none());
    }

    #[test]
    fn pin_messages() {
        let r = vec![HttpsRemote {
            name: "origin".into(),
            url: "https://github.com/o/r".into(),
            ssh_url: "git@github.com:o/r.git".into(),
        }];
        assert!(pin_message(&HttpsAction::Nothing, "P").is_none());
        let m = pin_message(&HttpsAction::Switched(r.clone()), "Personal").unwrap();
        assert_eq!(
            m.text,
            "GitHop: switched origin to SSH, so pushes use Personal's key"
        );
        let m = pin_message(&HttpsAction::Ask(r), "Personal").unwrap();
        assert!(m.text.contains("Uses HTTPS: Personal's SSH key isn't used"));
        assert_eq!(m.hint.as_deref(), Some("Fix: githop remote ssh"));
    }

    #[test]
    fn names_join() {
        let mk = |n: &str| HttpsRemote {
            name: n.into(),
            url: String::new(),
            ssh_url: String::new(),
        };
        assert_eq!(names_and(&[mk("origin")]), "origin");
        assert_eq!(names_and(&[mk("a"), mk("b"), mk("c")]), "a, b and c");
    }
}
