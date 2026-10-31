//! `check --stage push` (pre-push) — Phase 3 §3.3, Plan §7.4.
//!
//! 1. Parse stdin `<local ref> <local sha> <remote ref> <remote sha>`; skip deletes.
//! 2. **One** `git log` for all refs: positives = local shas, negatives = known
//!    remote shas (`^sha`), plus `--not --remotes=<remote>` when a ref is new.
//!    If git rejects a remote sha it doesn't have (force push over unfetched
//!    work), retry once with `--not --remotes=<remote>` instead.
//! 3. Compare author **and** committer email (case-insensitive) with the owning
//!    profile's email; committer `noreply@github.com` is allowed (web-flow).
//! 4. Cap at [`MAX_COMMITS`]; list up to [`MAX_LISTED`] offenders.
//! 5. Warn (never block) when the SSH key in use isn't the profile's key.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::gitconfig::ConfigSnapshot;
use crate::pin;
use crate::profile::{Profile, ProfileSet};
use crate::resolver;
use crate::settings::EnforceMode;

use super::{CheckContext, CheckResult, Message, Outcome, Stage};

/// Most commits one push check looks at.
pub const MAX_COMMITS: usize = 5000;
/// Most offending commits listed in the message (all are in [`CheckResult::offending`]).
pub const MAX_LISTED: usize = 10;
/// Committer emails that never count as a mismatch (GitHub web-flow merges pulled locally).
pub const ALLOWED_COMMITTERS: [&str; 1] = ["noreply@github.com"];
/// `git log` format: hash, author email, committer email, subject; `-z` separates.
pub const LOG_FORMAT: &str = "--format=%H%x00%ae%x00%ce%x00%s";

/// One line of pre-push stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefUpdate {
    pub local_ref: String,
    pub local_sha: String,
    pub remote_ref: String,
    pub remote_sha: String,
}

impl RefUpdate {
    /// The remote doesn't have this ref yet.
    pub fn is_new(&self) -> bool {
        is_null_sha(&self.remote_sha)
    }
}

/// All-zero object id (SHA-1: 40, SHA-256: 64).
pub fn is_null_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b == b'0')
}

fn is_hex_oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse pre-push stdin. Deletes (local sha null) and malformed lines are skipped.
pub fn parse_prepush_stdin(stdin: &str) -> Vec<RefUpdate> {
    stdin
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let (lr, ls, rr, rs) = (it.next()?, it.next()?, it.next()?, it.next()?);
            if it.next().is_some() || !is_hex_oid(ls) || !is_hex_oid(rs) || is_null_sha(ls) {
                return None;
            }
            Some(RefUpdate {
                local_ref: lr.to_string(),
                local_sha: ls.to_ascii_lowercase(),
                remote_ref: rr.to_string(),
                remote_sha: rs.to_ascii_lowercase(),
            })
        })
        .collect()
}

/// Build the single `git log` argument list for `updates`.
///
/// `remote` = remote name for `--remotes=<name>` (`None` → all remote-tracking
/// refs, used when the push target is a URL rather than a named remote).
/// `fallback` = don't use remote shas as negatives; use `--not --remotes` instead.
/// Asks for `MAX_COMMITS + 1` so capping can be detected.
pub fn build_log_args(updates: &[RefUpdate], remote: Option<&str>, fallback: bool) -> Vec<String> {
    let mut args = vec![
        "log".to_string(),
        "-z".to_string(),
        "--no-show-signature".to_string(),
        "--no-color".to_string(),
        LOG_FORMAT.to_string(),
        format!("--max-count={}", MAX_COMMITS + 1),
    ];
    let mut pos: Vec<&str> = Vec::new();
    let mut neg: Vec<&str> = Vec::new();
    let mut need_remotes = fallback;
    for u in updates {
        if !pos.contains(&u.local_sha.as_str()) {
            pos.push(&u.local_sha);
        }
        if u.is_new() {
            need_remotes = true;
        } else if !fallback && !neg.contains(&u.remote_sha.as_str()) {
            neg.push(&u.remote_sha);
        }
    }
    args.extend(pos.iter().map(|s| s.to_string()));
    args.extend(neg.iter().map(|s| format!("^{s}")));
    if need_remotes {
        args.push("--not".to_string());
        args.push(match remote {
            Some(r) => format!("--remotes={r}"),
            None => "--remotes".to_string(),
        });
    }
    args
}

/// One commit from `git log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogCommit {
    pub sha: String,
    pub author_email: String,
    pub committer_email: String,
    pub subject: String,
}

/// Parse `git log -z` output in [`LOG_FORMAT`]: NUL-separated fields, 4 per
/// commit. A trailing partial record is dropped.
pub fn parse_log_output(raw: &[u8]) -> Vec<LogCommit> {
    let text = String::from_utf8_lossy(raw);
    let mut fields = text.split('\0');
    let mut out = Vec::new();
    while let Some(sha) = fields.next() {
        let sha = sha.trim_start_matches('\n').trim();
        if sha.is_empty() {
            continue;
        }
        let (Some(ae), Some(ce), Some(subject)) = (fields.next(), fields.next(), fields.next())
        else {
            break;
        };
        out.push(LogCommit {
            sha: sha.to_string(),
            author_email: ae.trim().to_string(),
            committer_email: ce.trim().to_string(),
            subject: subject.trim_end_matches('\n').to_string(),
        });
    }
    out
}

/// A commit whose stamp doesn't match the repo's profile. Phase 4 `fix` consumes these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffendingCommit {
    pub sha: String,
    pub subject: String,
    pub author_email: String,
    pub committer_email: String,
    pub author_mismatch: bool,
    pub committer_mismatch: bool,
}

impl OffendingCommit {
    /// The email to show (author first).
    pub fn wrong_email(&self) -> &str {
        if self.author_mismatch {
            &self.author_email
        } else {
            &self.committer_email
        }
    }
}

fn email_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Commits whose author or committer email isn't `expected`. Pure.
pub fn find_offenders(commits: &[LogCommit], expected: &str) -> Vec<OffendingCommit> {
    commits
        .iter()
        .filter_map(|c| {
            let author_mismatch = !email_eq(&c.author_email, expected);
            let committer_mismatch = !email_eq(&c.committer_email, expected)
                && !ALLOWED_COMMITTERS
                    .iter()
                    .any(|a| email_eq(a, &c.committer_email));
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Messages for a blocked (or warned) push. Pure.
///
/// Headline per Design §9.3: `GitHop: push blocked — 2 commits use the Office
/// email in a Personal repo.` When the wrong emails don't all belong to one
/// other profile: `… 2 commits don't use the Personal email.`
pub fn mismatch_messages(
    owner: &Profile,
    set: &ProfileSet,
    offenders: &[OffendingCommit],
    mode: EnforceMode,
) -> Vec<Message> {
    let n = offenders.len();
    let mut other: Option<&Profile> = None;
    let mut single_other = true;
    for o in offenders {
        let p = set
            .profiles
            .iter()
            .find(|p| email_eq(&p.email, o.wrong_email()));
        match (p, other) {
            (Some(p), None) => other = Some(p),
            (Some(p), Some(q)) if p.id == q.id => {}
            _ => single_other = false,
        }
    }
    let commits = if n == 1 {
        "1 commit".to_string()
    } else {
        format!("{n} commits")
    };
    let what = match other.filter(|_| single_other) {
        Some(p) => format!(
            "{commits} {} the {} email in a {} repo.",
            if n == 1 { "uses" } else { "use" },
            p.label,
            owner.label
        ),
        None => format!(
            "{commits} {} use the {} email.",
            if n == 1 { "doesn't" } else { "don't" },
            owner.label
        ),
    };
    let mut out = vec![if mode == EnforceMode::Warn {
        Message::warning(format!("GitHop: push went through, but {what}"))
    } else {
        Message::blocked(format!("GitHop: push blocked — {what}"))
    }];
    for o in offenders.iter().take(MAX_LISTED) {
        out.push(Message::info(format!(
            "  {}  {}  {}",
            o.sha.get(..7).unwrap_or(&o.sha),
            truncate(&o.subject, 40),
            o.wrong_email()
        )));
    }
    if n > MAX_LISTED {
        out.push(Message::info(format!("  …and {} more", n - MAX_LISTED)));
    }
    if let Some(last) = out.last_mut() {
        last.hint = Some(if mode == EnforceMode::Warn {
            "To re-stamp them: githop fix".to_string()
        } else {
            "Run: githop fix".to_string()
        });
    }
    out
}

/// Message when the scan was capped.
pub fn capped_message() -> Message {
    Message::warning(format!(
        "GitHop checked only the newest {MAX_COMMITS} commits in this push."
    ))
}

fn norm_key(s: &str) -> String {
    s.trim().trim_matches('"').replace('\\', "/").to_lowercase()
}

/// SSH key warning (warn only, Plan §7.4 step 4). `ssh_command` = the effective
/// command (`GIT_SSH_COMMAND` env, else `core.sshCommand`). `remote_url` = `$2`
/// of pre-push; https remotes don't use SSH, so no warning. Pure.
pub fn ssh_key_warning(
    owner: &Profile,
    ssh_command: Option<&str>,
    from_env: bool,
    remote_url: Option<&str>,
) -> Option<Message> {
    if remote_url.is_some_and(|u| {
        let u = u.trim().to_ascii_lowercase();
        u.starts_with("https://") || u.starts_with("http://") || u.starts_with("file://")
    }) {
        return None;
    }
    let cmd = ssh_command?;
    let key = pin::key_from_ssh_command(cmd)?;
    let expected = crate::paths::to_slash(&owner.ssh_key);
    if norm_key(&key) == norm_key(&expected) {
        return None;
    }
    let msg = Message::warning(format!(
        "GitHop: this push uses a different SSH key than {} does.",
        owner.label
    ));
    Some(if from_env {
        msg.hint("GIT_SSH_COMMAND is set. Unset it to use the profile's key.")
    } else {
        msg.hint(format!("Run: githop pin {}", owner.id))
    })
}

/// Orchestrator. `args` = `[remote name, remote url]`.
pub(crate) fn run(
    ctx: &CheckContext,
    args: &[String],
    stdin: &str,
    enforce: EnforceMode,
) -> Result<CheckResult> {
    let mut result = CheckResult::new(Stage::Push, enforce, Outcome::Pass);
    let updates = parse_prepush_stdin(stdin);
    if updates.is_empty() {
        return Ok(result);
    }
    let root = super::repo_root(ctx)?;
    let snapshot = ConfigSnapshot::read(&ctx.git, &root)?;
    let mut store = super::open_store(ctx)?;
    let aliases = store.settings().github_host_aliases.clone();
    let resolution = resolver::resolve(&snapshot, &root, store.profiles(), &aliases);
    let Some(owner) =
        super::resolved_profile(store.profiles(), resolution.profile_id.as_deref()).cloned()
    else {
        result.outcome = Outcome::Unresolved;
        if super::first_hint(&mut store, &root) {
            result.messages.push(super::unresolved_message());
        }
        return Ok(result);
    };
    result.profile_id = Some(owner.id.clone());

    let remote_name = args
        .first()
        .map(String::as_str)
        .filter(|n| snapshot.remotes().iter().any(|r| r == n));
    let remote_url = args.get(1).map(String::as_str);

    let raw = {
        let a = build_log_args(&updates, remote_name, false);
        let refs: Vec<&str> = a.iter().map(String::as_str).collect();
        match ctx.git.run(&root, &refs) {
            Ok(out) => out,
            Err(first) => {
                let fb = build_log_args(&updates, remote_name, true);
                if fb == a {
                    return Err(first);
                }
                let refs: Vec<&str> = fb.iter().map(String::as_str).collect();
                ctx.git.run(&root, &refs)?
            }
        }
    };
    let mut commits = parse_log_output(&raw);
    if commits.len() > MAX_COMMITS {
        commits.truncate(MAX_COMMITS);
        result.capped = true;
        result.messages.push(capped_message());
    }
    let offenders = find_offenders(&commits, &owner.email);
    if !offenders.is_empty() {
        result.messages.extend(mismatch_messages(
            &owner,
            store.profiles(),
            &offenders,
            enforce,
        ));
        result.offending = offenders;
        result.outcome = Outcome::Mismatch { pinned_now: None };
    }

    let env_ssh = ctx
        .git
        .env_var("GIT_SSH_COMMAND")
        .map(|v| v.to_string_lossy().to_string())
        .filter(|v| !v.trim().is_empty());
    let (cmd, from_env) = match env_ssh {
        Some(v) => (Some(v), true),
        None => (snapshot.get("core.sshCommand").map(str::to_string), false),
    };
    if let Some(w) = ssh_key_warning(&owner, cmd.as_deref(), from_env, remote_url) {
        result.messages.push(w);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::check::Level;
    use crate::profile::ProfileColor;

    const Z40: &str = "0000000000000000000000000000000000000000";
    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";

    fn profile(id: &str, label: &str, email: &str) -> Profile {
        Profile {
            id: id.into(),
            label: label.into(),
            name: label.into(),
            email: email.into(),
            color: ProfileColor::Blue,
            ssh_key: PathBuf::from(format!("C:/Users/me/.ssh/githop_{id}")),
            owners: vec![],
            folders: vec![],
        }
    }

    fn set() -> ProfileSet {
        ProfileSet {
            version: 1,
            default: None,
            profiles: vec![
                profile("office", "Office", "me@office.example"),
                profile("personal", "Personal", "me@personal.example"),
            ],
        }
    }

    #[test]
    fn null_sha_both_lengths() {
        assert!(is_null_sha(Z40));
        assert!(is_null_sha(&"0".repeat(64)));
        assert!(!is_null_sha("000"));
        assert!(!is_null_sha(A));
    }

    #[test]
    fn parse_stdin_skips_deletes_and_garbage() {
        let input = format!(
            "refs/heads/main {A} refs/heads/main {B}\n\
             refs/heads/new {C} refs/heads/new {Z40}\n\
             (delete) {Z40} refs/heads/old {B}\n\
             garbage line\n\
             \n\
             refs/heads/x {A} refs/heads/x\n\
             refs/heads/s256 {} refs/heads/s256 {}\r\n",
            "a".repeat(64),
            "0".repeat(64)
        );
        let u = parse_prepush_stdin(&input);
        assert_eq!(u.len(), 3);
        assert_eq!(u[0].local_sha, A);
        assert_eq!(u[0].remote_sha, B);
        assert!(!u[0].is_new());
        assert!(u[1].is_new());
        assert_eq!(u[2].local_sha, "a".repeat(64));
        assert!(u[2].is_new());
    }

    #[test]
    fn log_args_single_invocation() {
        let u = parse_prepush_stdin(&format!(
            "refs/heads/main {A} refs/heads/main {B}\nrefs/heads/new {C} refs/heads/new {Z40}\n"
        ));
        let a = build_log_args(&u, Some("origin"), false);
        assert_eq!(
            &a[..6],
            &[
                "log",
                "-z",
                "--no-show-signature",
                "--no-color",
                LOG_FORMAT,
                "--max-count=5001"
            ]
        );
        assert_eq!(
            &a[6..],
            &[
                A.to_string(),
                C.to_string(),
                format!("^{B}"),
                "--not".into(),
                "--remotes=origin".into()
            ]
        );

        // Existing branch only: plain range, no --not.
        let u1 = &u[..1];
        assert_eq!(
            &build_log_args(u1, Some("origin"), false)[6..],
            &[A.to_string(), format!("^{B}")]
        );
        // Fallback replaces remote shas with --not --remotes.
        assert_eq!(
            &build_log_args(u1, Some("origin"), true)[6..],
            &[A.to_string(), "--not".into(), "--remotes=origin".into()]
        );
        // Unknown remote name (URL push) → all remotes.
        assert_eq!(
            build_log_args(&u[1..], None, false).last().unwrap(),
            "--remotes"
        );
    }

    #[test]
    fn log_output_parsing() {
        let raw = format!(
            "{A}\0me@office.example\0me@office.example\0first one\0{B}\0Me@Personal.example\0noreply@github.com\0second\0"
        );
        let c = parse_log_output(raw.as_bytes());
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].subject, "first one");
        assert_eq!(c[1].author_email, "Me@Personal.example");
        // Without trailing NUL and with an empty subject.
        let c = parse_log_output(format!("{A}\0a@b\0a@b\0").as_bytes());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].subject, "");
        // Truncated record dropped.
        assert!(parse_log_output(format!("{A}\0a@b").as_bytes()).is_empty());
        assert!(parse_log_output(b"").is_empty());
    }

    fn lc(sha: &str, ae: &str, ce: &str) -> LogCommit {
        LogCommit {
            sha: sha.into(),
            author_email: ae.into(),
            committer_email: ce.into(),
            subject: format!("subject {sha}"),
        }
    }

    #[test]
    fn offenders_case_insensitive_and_noreply_allowed() {
        let commits = vec![
            lc(A, "ME@office.example", "me@office.example"),
            lc(B, "me@office.example", "noreply@github.com"),
            lc(C, "me@personal.example", "me@office.example"),
        ];
        let o = find_offenders(&commits, "me@office.example");
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].sha, C);
        assert!(o[0].author_mismatch && !o[0].committer_mismatch);
        // noreply is only allowed as committer.
        let o = find_offenders(
            &[lc(A, "noreply@github.com", "me@office.example")],
            "me@office.example",
        );
        assert_eq!(o.len(), 1);
        // committer-only mismatch
        let o = find_offenders(
            &[lc(A, "me@office.example", "me@personal.example")],
            "me@office.example",
        );
        assert_eq!(o[0].wrong_email(), "me@personal.example");
    }

    #[test]
    fn blocked_message_matches_design() {
        let s = set();
        let owner = s.get("personal").unwrap();
        let o = find_offenders(
            &[
                lc(A, "me@office.example", "me@office.example"),
                lc(B, "me@office.example", "me@office.example"),
            ],
            &owner.email,
        );
        let m = mismatch_messages(owner, &s, &o, EnforceMode::Block);
        assert_eq!(m[0].level, Level::Blocked);
        assert_eq!(
            m[0].text,
            "GitHop: push blocked — 2 commits use the Office email in a Personal repo."
        );
        assert_eq!(m.len(), 3);
        assert!(m[1].text.contains("1111111"));
        assert!(m[1].text.contains("me@office.example"));
        assert_eq!(m.last().unwrap().hint.as_deref(), Some("Run: githop fix"));
    }

    #[test]
    fn message_for_unknown_emails_and_singular() {
        let s = set();
        let owner = s.get("office").unwrap();
        let o = find_offenders(
            &[lc(A, "who@else.example", "who@else.example")],
            &owner.email,
        );
        let m = mismatch_messages(owner, &s, &o, EnforceMode::Block);
        assert_eq!(
            m[0].text,
            "GitHop: push blocked — 1 commit doesn't use the Office email."
        );
        let w = mismatch_messages(owner, &s, &o, EnforceMode::Warn);
        assert_eq!(w[0].level, Level::Warning);
        assert!(w.iter().all(|m| m.level != Level::Blocked));
    }

    #[test]
    fn list_capped_at_ten() {
        let s = set();
        let owner = s.get("office").unwrap();
        let commits: Vec<_> = (0..13)
            .map(|i| {
                lc(
                    &format!("{i:040}"),
                    "me@personal.example",
                    "me@personal.example",
                )
            })
            .collect();
        let o = find_offenders(&commits, &owner.email);
        assert_eq!(o.len(), 13);
        let m = mismatch_messages(owner, &s, &o, EnforceMode::Block);
        assert_eq!(m.len(), 1 + 10 + 1);
        assert_eq!(m.last().unwrap().text, "  …and 3 more");
    }

    #[test]
    fn capped_message_text() {
        assert_eq!(
            capped_message().text,
            "GitHop checked only the newest 5000 commits in this push."
        );
    }

    #[test]
    fn truncate_long_subject() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn ssh_warning_skipped_for_https_and_missing_command() {
        let s = set();
        let owner = s.get("office").unwrap();
        assert_eq!(
            ssh_key_warning(owner, None, false, Some("git@github.com:o/r.git")),
            None
        );
        assert_eq!(
            ssh_key_warning(
                owner,
                Some("ssh -i x"),
                false,
                Some("https://github.com/o/r.git")
            ),
            None
        );
    }
}
