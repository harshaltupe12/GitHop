//! `check --stage push` (pre-push) — Phase 3 §3.3, Plan §7.4.
//!
//! 1. Parse stdin `<local ref> <local sha> <remote ref> <remote sha>`; skip deletes.
//! 2. **One** `git log` for all refs: positives = local shas, negatives = known
//!    remote shas (`^sha`), plus `--not --remotes=<remote>` when a ref is new.
//!    If git rejects a remote sha it doesn't have (force push over unfetched
//!    work), retry once with `--not --remotes=<remote>` instead.
//! 3. Author **and** committer email are checked with [`IdentityRules`]: an
//!    email is wrong only when it is one of the *user's* identities and not the
//!    owning profile's (case-insensitive). Someone else's commits (a merged PR,
//!    `git am`) never block; committer `noreply@github.com` is allowed (web-flow).
//! 4. Cap at [`MAX_COMMITS`]; list up to [`MAX_LISTED`] offenders.
//! 5. Annotated tags among the pushed refs (`refs/tags/*` whose local sha is a
//!    tag object): the tagger email is checked with the same rule. This costs a
//!    third spawn (`for-each-ref`), **only** when the push includes a tag.
//! 6. Warn (never block) when the SSH key in use isn't the profile's key.
//! 7. When the push will go through (exit 0), append the pushed local shas to
//!    [`PUSHED_FILE`] in the repo's common git dir (no spawn). `fix` treats
//!    everything reachable from a recorded sha as pushed. Conservative: a push
//!    that fails later still counts as pushed.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::gitconfig::ConfigSnapshot;
use crate::pin;
use crate::profile::{Profile, ProfileSet};
use crate::resolver;
use crate::settings::EnforceMode;

use super::identity_rules::IdentityRules;
use super::{CheckContext, CheckResult, Message, Outcome, Stage};

pub use super::identity_rules::{find_offenders, ALLOWED_COMMITTERS};

/// Most commits one push check looks at.
pub const MAX_COMMITS: usize = 5000;
/// Most offending commits listed in the message (all are in [`CheckResult::offending`]).
pub const MAX_LISTED: usize = 10;
/// Pushed-sha record, relative to the repo's common git dir. One line per
/// pushed ref: `<local sha>\t<remote ref>\t<remote name or URL>`.
pub const PUSHED_FILE: &str = "githop/pushed";
/// Lines kept in [`PUSHED_FILE`] (oldest dropped first).
pub const PUSHED_KEEP: usize = 2000;
/// [`PUSHED_FILE`] is only re-read for trimming once it is bigger than this.
const PUSHED_TRIM_BYTES: u64 = 128 * 1024;
/// Pushing more tags than this queries all local tags instead of each by name.
const MAX_TAG_PATTERNS: usize = 64;
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
    let commits = if n == 1 {
        "1 commit".to_string()
    } else {
        format!("{n} commits")
    };
    let what = match single_other_profile(set, offenders.iter().map(|o| o.wrong_email())) {
        Some(p) => format!(
            "{commits} {} the {} email in {} {} repo.",
            if n == 1 { "uses" } else { "use" },
            p.label,
            indefinite_article(&owner.label),
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

// ---------------------------------------------------------------- tags

/// An annotated tag in the push whose tagger email is wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffendingTag {
    /// Short tag name (`v1`).
    pub name: String,
    /// The tag object's id.
    pub sha: String,
    pub tagger_email: String,
}

/// `for-each-ref` format for tags: object id, type, tagger email, ref name.
pub const TAG_FORMAT: &str = "--format=%(objectname)%00%(objecttype)%00%(taggeremail)%00%(refname)";

/// Pushed updates whose local ref is a tag.
fn tag_updates(updates: &[RefUpdate]) -> Vec<&RefUpdate> {
    updates
        .iter()
        .filter(|u| u.local_ref.starts_with("refs/tags/"))
        .collect()
}

/// `for-each-ref` arguments for the tags in `updates` (`None` = no tag pushed).
pub fn build_tag_args(updates: &[RefUpdate]) -> Option<Vec<String>> {
    let tags = tag_updates(updates);
    if tags.is_empty() {
        return None;
    }
    let mut args = vec!["for-each-ref".to_string(), TAG_FORMAT.to_string()];
    if tags.len() > MAX_TAG_PATTERNS {
        args.push("refs/tags/".to_string());
    } else {
        let mut names: Vec<&str> = tags.iter().map(|u| u.local_ref.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        args.extend(names.iter().map(|n| n.to_string()));
    }
    Some(args)
}

/// Annotated tags in the push with a wrong tagger email. `raw` = output of
/// [`build_tag_args`]; only tags whose object id is a pushed local sha count. Pure.
pub fn find_tag_offenders(
    raw: &[u8],
    updates: &[RefUpdate],
    rules: &IdentityRules,
) -> Vec<OffendingTag> {
    let text = String::from_utf8_lossy(raw);
    let pushed = tag_updates(updates);
    let mut out: Vec<OffendingTag> = Vec::new();
    for line in text.lines() {
        let mut f = line.split('\0');
        let (Some(sha), Some(kind), Some(email), Some(refname)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let sha = sha.trim().to_ascii_lowercase();
        if kind != "tag"
            || !pushed
                .iter()
                .any(|u| u.local_sha == sha && u.local_ref == refname)
        {
            continue;
        }
        let email = email.trim().trim_start_matches('<').trim_end_matches('>');
        if rules.author_wrong(email) && !out.iter().any(|t| t.sha == sha) {
            out.push(OffendingTag {
                name: refname
                    .strip_prefix("refs/tags/")
                    .unwrap_or(refname)
                    .to_string(),
                sha,
                tagger_email: email.to_string(),
            });
        }
    }
    out
}

/// The one other profile every email in `emails` belongs to, if there is one.
fn single_other_profile<'a, 'e>(
    set: &'a ProfileSet,
    emails: impl IntoIterator<Item = &'e str>,
) -> Option<&'a Profile> {
    let mut other: Option<&Profile> = None;
    for e in emails {
        let p = set.profiles.iter().find(|p| email_eq(&p.email, e))?;
        match other {
            None => other = Some(p),
            Some(q) if q.id == p.id => {}
            Some(_) => return None,
        }
    }
    other
}

/// "a" / "an" by the label's first letter ("an Office repo").
fn indefinite_article(label: &str) -> &'static str {
    match label
        .trim_start()
        .chars()
        .next()
        .map(|c| c.to_ascii_lowercase())
    {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    }
}

/// Messages for annotated tags with a wrong tagger email. Pure.
///
/// `GitHop: push blocked — tag v1 uses the Personal email in an Office repo.`
pub fn tag_messages(
    owner: &Profile,
    set: &ProfileSet,
    tags: &[OffendingTag],
    mode: EnforceMode,
) -> Vec<Message> {
    let n = tags.len();
    let subject = match tags {
        [t] => format!("tag {}", t.name),
        _ => format!("{n} tags"),
    };
    let what = match single_other_profile(set, tags.iter().map(|t| t.tagger_email.as_str())) {
        Some(p) => format!(
            "{subject} {} the {} email in {} {} repo.",
            if n == 1 { "uses" } else { "use" },
            p.label,
            indefinite_article(&owner.label),
            owner.label
        ),
        None => format!(
            "{subject} {} use the {} email.",
            if n == 1 { "doesn't" } else { "don't" },
            owner.label
        ),
    };
    let mut out = vec![if mode == EnforceMode::Warn {
        Message::warning(format!("GitHop: push went through, but {what}"))
    } else {
        Message::blocked(format!("GitHop: push blocked — {what}"))
    }];
    for t in tags.iter().take(MAX_LISTED) {
        out.push(Message::info(format!("  {}  {}", t.name, t.tagger_email)));
    }
    if n > MAX_LISTED {
        out.push(Message::info(format!("  …and {} more", n - MAX_LISTED)));
    }
    if let Some(last) = out.last_mut() {
        last.hint = Some(match tags {
            [t] => format!("Run: git tag -f -a {0} {0}^{{}}", t.name),
            _ => "Re-create each tag: git tag -f -a <tag> <tag>^{}".to_string(),
        });
    }
    out
}

// ------------------------------------------------------- pushed record

/// One line of [`PUSHED_FILE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedEntry {
    /// Local sha that was pushed (commit or tag object).
    pub sha: String,
    /// Remote ref it was pushed to (`refs/heads/feature`).
    pub remote_ref: String,
    /// Remote name or URL (pre-push `$1`).
    pub remote: String,
}

/// `<common dir>/githop/pushed`.
pub fn pushed_record_path(common_dir: &Path) -> PathBuf {
    common_dir.join(PUSHED_FILE)
}

/// Record lines for `updates` (deletes are already gone). Pure.
pub fn pushed_lines(remote: &str, updates: &[RefUpdate]) -> String {
    let remote: String = remote.chars().filter(|c| !c.is_control()).collect();
    updates
        .iter()
        .map(|u| format!("{}\t{}\t{remote}\n", u.local_sha, u.remote_ref))
        .collect()
}

/// Parse [`PUSHED_FILE`] content; malformed lines are skipped. Pure.
pub fn parse_pushed(text: &str) -> Vec<PushedEntry> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.trim_end_matches('\r').splitn(3, '\t');
            let sha = f.next()?.trim().to_ascii_lowercase();
            if !is_hex_oid(&sha) || is_null_sha(&sha) {
                return None;
            }
            Some(PushedEntry {
                sha,
                remote_ref: f.next().unwrap_or_default().to_string(),
                remote: f.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// Read the record of `common_dir` (missing file = empty). Bounded by
/// [`PUSHED_KEEP`] lines after trimming; reads at most 4 MiB.
pub fn read_pushed(common_dir: &Path) -> std::io::Result<Vec<PushedEntry>> {
    use std::io::Read;
    let path = pushed_record_path(common_dir);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut buf = Vec::new();
    file.take(4 * 1024 * 1024).read_to_end(&mut buf)?;
    Ok(parse_pushed(&String::from_utf8_lossy(&buf)))
}

/// Append `updates` to the record (one write, append mode, so concurrent
/// pushes don't clobber each other); trim to the last [`PUSHED_KEEP`] lines
/// once the file is big. No spawn.
pub fn record_pushed(common_dir: &Path, remote: &str, updates: &[RefUpdate]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let path = pushed_record_path(common_dir);
    let io = |e| crate::error::CoreError::io(&path, e);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let lines = pushed_lines(remote, updates);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(io)?;
    f.write_all(lines.as_bytes()).map_err(io)?;
    drop(f);
    let big = std::fs::metadata(&path).map(|m| m.len() > PUSHED_TRIM_BYTES);
    if big.unwrap_or(false) {
        let text = std::fs::read(&path).map_err(io)?;
        let text = String::from_utf8_lossy(&text);
        let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        if all.len() > PUSHED_KEEP {
            let kept: String = all[all.len() - PUSHED_KEEP..]
                .iter()
                .map(|l| format!("{l}\n"))
                .collect();
            crate::store::atomic_write(&path, kept.as_bytes())?;
        }
    }
    Ok(())
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
    let repo = super::hook_repo(ctx)?;
    let root = repo.root().to_path_buf();
    let remote_arg = args.first().map(String::as_str).unwrap_or_default();
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
        // The push goes through: remember what it publishes (best effort).
        let _ = record_pushed(repo.common_dir(), remote_arg, &updates);
        return Ok(result);
    };
    result.profile_id = Some(owner.id.clone());
    let rules = IdentityRules::for_repo(&ctx.git, &ctx.paths, store.profiles(), &owner, &snapshot);

    let remote_url = args.get(1).map(String::as_str);

    // New branches exclude commits reachable from *any* remote-tracking ref, not
    // just the push target's: those are already published (same rule as `fix`),
    // and in fork workflows `upstream` commits pushed to `origin` aren't ours.
    let remote_name = None;
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
    let offenders = find_offenders(&commits, &rules);
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

    // Annotated tags: one more spawn, only when the push includes a tag.
    if let Some(a) = build_tag_args(&updates) {
        let refs: Vec<&str> = a.iter().map(String::as_str).collect();
        match ctx.git.run(&root, &refs) {
            Ok(raw) => {
                let tags = find_tag_offenders(&raw, &updates, &rules);
                if !tags.is_empty() {
                    result
                        .messages
                        .extend(tag_messages(&owner, store.profiles(), &tags, enforce));
                    result.outcome = Outcome::Mismatch { pinned_now: None };
                }
            }
            // Don't lose the commit result over the tag lookup.
            Err(_) => result.messages.push(Message::warning(
                "GitHop couldn't check the tags in this push.",
            )),
        }
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
    if result.exit_code() == 0 {
        // Best effort: a failed record must never block or fail the push.
        let _ = record_pushed(repo.common_dir(), remote_arg, &updates);
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

    /// Rules for `owner`: both profile emails plus a global `user.email`.
    fn rules(owner: &str) -> IdentityRules {
        IdentityRules::new(
            owner,
            [
                "me@office.example",
                "me@personal.example",
                "global@me.example",
            ],
            None,
            None,
        )
    }

    fn upd(local_ref: &str, sha: &str) -> RefUpdate {
        RefUpdate {
            local_ref: local_ref.into(),
            local_sha: sha.into(),
            remote_ref: local_ref.into(),
            remote_sha: Z40.into(),
        }
    }

    #[test]
    fn foreign_commits_are_not_offenders() {
        let o = find_offenders(
            &[
                lc(A, "alice@contrib.example", "alice@contrib.example"),
                lc(B, "alice@contrib.example", "me@office.example"),
            ],
            &rules("me@office.example"),
        );
        assert!(o.is_empty(), "{o:?}");
    }

    #[test]
    fn tag_args_only_when_tags_are_pushed() {
        assert_eq!(build_tag_args(&[upd("refs/heads/main", A)]), None);
        let a = build_tag_args(&[
            upd("refs/heads/main", A),
            upd("refs/tags/v2", B),
            upd("refs/tags/v1", C),
        ])
        .unwrap();
        assert_eq!(a[..2], ["for-each-ref".to_string(), TAG_FORMAT.to_string()]);
        assert_eq!(a[2..], ["refs/tags/v1".to_string(), "refs/tags/v2".into()]);
        let many: Vec<RefUpdate> = (0..=MAX_TAG_PATTERNS)
            .map(|i| upd(&format!("refs/tags/t{i}"), A))
            .collect();
        assert_eq!(
            build_tag_args(&many).unwrap()[2..],
            ["refs/tags/".to_string()]
        );
    }

    #[test]
    fn tag_offenders_need_a_pushed_annotated_tag_with_the_users_email() {
        let updates = [upd("refs/tags/v1", A), upd("refs/tags/v2", B)];
        let raw = format!(
            "{A}\0tag\0<me@personal.example>\0refs/tags/v1\n\
             {B}\0tag\0<alice@contrib.example>\0refs/tags/v2\n\
             {C}\0tag\0<me@personal.example>\0refs/tags/v3\n\
             {C}\0commit\0\0refs/tags/light\n"
        );
        let t = find_tag_offenders(raw.as_bytes(), &updates, &rules("me@office.example"));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "v1");
        assert_eq!(t[0].tagger_email, "me@personal.example");
        let s = set();
        let m = tag_messages(s.get("office").unwrap(), &s, &t, EnforceMode::Block);
        assert_eq!(
            m[0].text,
            "GitHop: push blocked — tag v1 uses the Personal email in an Office repo."
        );
        assert_eq!(
            m.last().unwrap().hint.as_deref(),
            Some("Run: git tag -f -a v1 v1^{}")
        );
    }

    #[test]
    fn pushed_record_roundtrip_and_trim() {
        let tmp = tempfile::tempdir().unwrap();
        let u = [upd("refs/heads/feature", A), upd("refs/tags/v1", B)];
        record_pushed(tmp.path(), "origin", &u).unwrap();
        record_pushed(tmp.path(), "C:/my\tremotes/x.git", &u[..1]).unwrap();
        let e = read_pushed(tmp.path()).unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!(
            e[0],
            PushedEntry {
                sha: A.into(),
                remote_ref: "refs/heads/feature".into(),
                remote: "origin".into()
            }
        );
        assert_eq!(e[2].remote, "C:/myremotes/x.git", "control chars dropped");
        // Garbage lines are skipped.
        assert_eq!(parse_pushed("nope\n\n0000\tx\ty\n").len(), 0);
        // Bounded: only the newest PUSHED_KEEP lines survive.
        let many: Vec<RefUpdate> = (0..PUSHED_KEEP + 50)
            .map(|i| upd("refs/heads/x", &format!("{i:040x}")))
            .collect();
        record_pushed(tmp.path(), "origin-with-a-long-name", &many).unwrap();
        let e = read_pushed(tmp.path()).unwrap();
        assert_eq!(e.len(), PUSHED_KEEP);
        assert_eq!(e.last().unwrap().sha, format!("{:040x}", PUSHED_KEEP + 49));
        assert!(read_pushed(&tmp.path().join("missing")).unwrap().is_empty());
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
        let o = find_offenders(&commits, &rules("me@office.example"));
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].sha, C);
        assert!(o[0].author_mismatch && !o[0].committer_mismatch);
        // noreply as author isn't one of the user's emails, so not an offender
        // (the "only as committer" exemption matters only when it is the user's:
        // see identity_rules::noreply_committer_allowed_but_not_as_author).
        let o = find_offenders(
            &[lc(A, "noreply@github.com", "me@office.example")],
            &rules("me@office.example"),
        );
        assert!(o.is_empty());
        // committer-only mismatch
        let o = find_offenders(
            &[lc(A, "me@office.example", "me@personal.example")],
            &rules("me@office.example"),
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
            &rules(&owner.email),
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
            &[lc(A, "global@me.example", "global@me.example")],
            &rules(&owner.email),
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
        let o = find_offenders(&commits, &rules(&owner.email));
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
