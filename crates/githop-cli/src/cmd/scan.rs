//! `githop scan <folder> [--apply] [--json]` — find repos and show which profile owns each.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use githop_core::paths::normalize_repo_root;
use githop_core::scan::{self, ScannedRepo};
use githop_core::text::{plural, wrap};
use githop_core::{pin as core_pin, ProfileSet, Repo, ResolutionSource};
use serde_json::json;

use super::{
    add_owner_command, labels_and, ownerless_profiles, remember_repo, source_tag, unmatched_owner,
    warning_json, Ctx,
};
use crate::cli::ScanArgs;
use crate::exit::{usage, Exit};

/// Per-repo status shown in the table and JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoStatus {
    Error,
    Pinned,
    StalePin,
    Unresolved,
    WrongIdentity,
    NotPinned,
    PinnedNow,
}

impl RepoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RepoStatus::Error => "can't read",
            RepoStatus::Pinned => "pinned",
            RepoStatus::StalePin => "stale pin",
            RepoStatus::Unresolved => "unresolved",
            RepoStatus::WrongIdentity => "wrong identity",
            RepoStatus::NotPinned => "not pinned",
            RepoStatus::PinnedNow => "pinned now",
        }
    }

    /// Stable machine name for JSON.
    pub fn key(self) -> &'static str {
        match self {
            RepoStatus::Error => "error",
            RepoStatus::Pinned => "pinned",
            RepoStatus::StalePin => "stale-pin",
            RepoStatus::Unresolved => "unresolved",
            RepoStatus::WrongIdentity => "wrong-identity",
            RepoStatus::NotPinned => "not-pinned",
            RepoStatus::PinnedNow => "pinned-now",
        }
    }
}

/// Classify one scanned repo.
pub fn classify(r: &ScannedRepo, set: &ProfileSet) -> RepoStatus {
    if r.error.is_some() {
        return RepoStatus::Error;
    }
    let res = &r.resolution;
    if res.source == Some(ResolutionSource::Pin) {
        return RepoStatus::Pinned;
    }
    if res.pinned.is_some() {
        return RepoStatus::StalePin;
    }
    let Some(profile) = res.profile_id.as_deref().and_then(|id| set.get(id)) else {
        return RepoStatus::Unresolved;
    };
    match &r.identity {
        Some(i)
            if !(i.author.email_matches(&profile.email)
                && i.committer.email_matches(&profile.email)) =>
        {
            RepoStatus::WrongIdentity
        }
        _ => RepoStatus::NotPinned,
    }
}

/// Can `--apply` pin this repo?
fn pinnable(r: &ScannedRepo, status: RepoStatus, include_default: bool) -> bool {
    matches!(status, RepoStatus::NotPinned | RepoStatus::WrongIdentity)
        && r.resolution.profile_id.is_some()
        && (include_default || r.resolution.source != Some(ResolutionSource::Default))
}

/// `root` shown relative to the scanned folder.
fn relative(root: &str, base: &str) -> String {
    let root_slash = if root.ends_with('/') {
        root.to_string()
    } else {
        format!("{root}/")
    };
    let head = root_slash.get(..base.len());
    let tail = root_slash.get(base.len()..);
    match (head, tail) {
        (Some(h), Some(rest)) if h.to_lowercase() == base.to_lowercase() => {
            let rest = rest.trim_end_matches('/');
            if rest.is_empty() {
                ".".to_string()
            } else {
                rest.to_string()
            }
        }
        _ => root.trim_end_matches('/').to_string(),
    }
}

pub fn run(ctx: &Ctx, args: ScanArgs) -> Result<Exit> {
    let ui = &ctx.ui;
    if !args.folder.is_dir() {
        return Err(usage(format!(
            "There's no folder at {}.",
            githop_core::paths::to_slash(&args.folder)
        )));
    }
    let git = ctx.git()?;
    let mut store = ctx.store()?;
    // Cache SSH aliases for github.com (`ssh -G`) so owner rules work with them;
    // hooks read the cache. Best effort.
    let _ = githop_core::doctor::refresh_github_host_aliases(&mut store, git);
    let set = store.profiles().clone();
    let mut settings = store.settings().clone();
    let base = normalize_repo_root(&args.folder);

    let repos = scan::scan(
        git,
        &args.folder,
        &set,
        &settings.github_host_aliases,
        args.depth,
    );
    let mut statuses: Vec<RepoStatus> = repos.iter().map(|r| classify(r, &set)).collect();

    let mut applied: Vec<String> = Vec::new();
    let mut apply_errors: Vec<(String, String)> = Vec::new();
    let mut pinned_now: Vec<Repo> = Vec::new();
    if args.apply {
        for (r, status) in repos.iter().zip(statuses.iter_mut()) {
            if !pinnable(r, *status, args.include_default) {
                continue;
            }
            let Some(profile) = r
                .resolution
                .profile_id
                .as_deref()
                .and_then(|id| set.get(id))
            else {
                continue;
            };
            let pinned = Repo::discover(git, Path::new(&r.root))
                .and_then(|repo| core_pin::pin_repo(&repo, profile).map(|()| repo));
            match pinned {
                Ok(repo) => {
                    remember_repo(&mut settings.pinned_repos, repo.root());
                    *status = RepoStatus::PinnedNow;
                    applied.push(r.root.clone());
                    pinned_now.push(repo);
                }
                Err(e) => apply_errors.push((r.root.clone(), e.to_string())),
            }
        }
        if !applied.is_empty() {
            let roots = settings.pinned_repos.clone();
            store.update_settings(|s| {
                for r in &roots {
                    remember_repo(&mut s.pinned_repos, Path::new(r));
                }
                Ok(())
            })?;
            githop_core::ssh::ensure_github_known_hosts_best_effort(git, store.paths());
        }
    }
    // ADR 0006: repos pinned now that still push over GitHub HTTPS.
    let mut switched_to_ssh: Vec<String> = Vec::new();
    let mut still_https: Vec<&Repo> = Vec::new();
    let policy = store.settings().remote_https_policy;
    for repo in &pinned_now {
        match githop_core::remote_ssh::apply_policy(repo, policy) {
            Ok(githop_core::remote_ssh::HttpsAction::Switched(l)) if !l.is_empty() => {
                switched_to_ssh.push(normalize_repo_root(repo.root()));
            }
            Ok(
                githop_core::remote_ssh::HttpsAction::Ask(_)
                | githop_core::remote_ssh::HttpsAction::Warn(_),
            ) => still_https.push(repo),
            _ => {}
        }
    }

    if ui.json {
        let items: Vec<serde_json::Value> = repos
            .iter()
            .zip(&statuses)
            .map(|(r, s)| {
                let warnings: Vec<serde_json::Value> = r
                    .resolution
                    .warnings
                    .iter()
                    .map(|w| warning_json(w, &set))
                    .collect();
                json!({
                    "root": r.root,
                    "status": s.key(),
                    "profile_id": r.resolution.profile_id,
                    "source": r.resolution.source,
                    "resolution": r.resolution,
                    "identity": r.identity,
                    "error": r.error,
                    "warnings": warnings,
                })
            })
            .collect();
        let errors: Vec<serde_json::Value> = apply_errors
            .iter()
            .map(|(root, e)| json!({ "root": root, "error": e }))
            .collect();
        ui.json_out(&json!({
            "folder": base,
            "repos": items,
            "applied": applied,
            "apply_errors": errors,
            "switched_to_ssh": switched_to_ssh,
            "still_https": still_https
                .iter()
                .map(|r| normalize_repo_root(r.root()))
                .collect::<Vec<_>>(),
        }))?;
        return Ok(Exit::Ok);
    }

    if repos.is_empty() {
        ui.line(&format!(
            "No git repos found in {base} (searched {} levels).",
            args.depth
        ));
        return Ok(Exit::Ok);
    }

    let rows: Vec<Vec<String>> = repos
        .iter()
        .zip(&statuses)
        .map(|(r, s)| {
            let profile = r
                .resolution
                .profile_id
                .as_deref()
                .map(|id| super::label_for(&set, id))
                .unwrap_or_else(|| "—".to_string());
            vec![
                relative(&r.root, &base),
                profile,
                source_tag(&r.resolution).to_string(),
                s.as_str().to_string(),
            ]
        })
        .collect();
    ui.table(&["Repo", "Profile", "Source", "Status"], &rows);

    for (r, s) in repos.iter().zip(&statuses) {
        let name = relative(&r.root, &base);
        if *s == RepoStatus::Error {
            ui.warn(&format!(
                "{name}: couldn't read this repo ({})",
                r.error.as_deref().unwrap_or("unknown error")
            ));
        }
        for w in &r.resolution.warnings {
            ui.warn(&wrap(
                &format!("{name}: {}", super::describe_warning(w, &set)),
                crate::ui::MAX_WIDTH - 2,
                "  ",
            ));
        }
    }
    for (root, e) in &apply_errors {
        ui.warn(&format!("{}: couldn't pin ({e})", relative(root, &base)));
    }

    let count = |want: &[RepoStatus]| statuses.iter().filter(|s| want.contains(s)).count();
    ui.line(&format!(
        "{}: {} pinned, {} not pinned, {} unresolved.",
        plural(repos.len(), "repo", "repos"),
        count(&[RepoStatus::Pinned, RepoStatus::PinnedNow]),
        count(&[
            RepoStatus::NotPinned,
            RepoStatus::WrongIdentity,
            RepoStatus::StalePin
        ]),
        count(&[RepoStatus::Unresolved]),
    ));
    if args.apply {
        ui.ok(&format!(
            "Pinned {}.",
            plural(applied.len(), "repo", "repos")
        ));
        if !switched_to_ssh.is_empty() {
            ui.ok(&format!(
                "Switched {} from HTTPS to SSH, so pushes use each profile's key.",
                plural(switched_to_ssh.len(), "repo", "repos")
            ));
        }
        if !still_https.is_empty() {
            let n = still_https.len();
            ui.warn(&wrap(
                &format!(
                    "{} {} GitHub over HTTPS, so Windows signs in with the account it saved, not the profile's SSH key.",
                    plural(n, "pinned repo", "pinned repos"),
                    if n == 1 { "pushes to" } else { "push to" }
                ),
                crate::ui::MAX_WIDTH - 2,
                "  ",
            ));
            let ask = policy == githop_core::RemoteHttpsPolicy::Ask && ui.interactive;
            if ask && ui.confirm("Switch them to SSH now?", true)? {
                let mut done = 0;
                for repo in &still_https {
                    if githop_core::remote_ssh::switch_all_to_ssh(repo).is_ok_and(|l| !l.is_empty())
                    {
                        done += 1;
                    }
                }
                ui.ok(&format!("Switched {}.", plural(done, "repo", "repos")));
                ui.hint("To always do this when pinning: githop remote policy auto");
            } else {
                ui.fix_hint(&format!(
                    "Run: githop remote ssh --all-in {}",
                    githop_core::paths::to_slash(&args.folder)
                ));
            }
        }
    }
    let waiting = repos
        .iter()
        .zip(&statuses)
        .filter(|(r, s)| pinnable(r, **s, args.include_default))
        .count();
    let default_only = repos
        .iter()
        .zip(&statuses)
        .filter(|(r, s)| {
            !args.include_default
                && pinnable(r, **s, true)
                && r.resolution.source == Some(ResolutionSource::Default)
        })
        .count();
    if !args.apply && waiting > 0 {
        ui.hint(&format!(
            "Pin them: githop scan {} --apply",
            githop_core::paths::to_slash(&args.folder)
        ));
    }
    if default_only > 0 {
        ui.hint(&wrap(
            &format!(
                "{} only matched the default profile and {} pinned. \
                 Add owner or folder rules, or use --include-default.",
                plural(default_only, "repo", "repos"),
                if default_only == 1 {
                    "wasn't"
                } else {
                    "weren't"
                }
            ),
            crate::ui::MAX_WIDTH,
            "",
        ));
    }
    for line in owner_summary(&repos, &statuses, &set) {
        match line {
            SummaryLine::Warn(t) => ui.warn(&wrap(&t, crate::ui::MAX_WIDTH - 2, "  ")),
            SummaryLine::Row(t) => ui.line(&t),
            SummaryLine::Hint(t) => ui.fix_hint(&t),
        }
    }
    Ok(Exit::Ok)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SummaryLine {
    Warn(String),
    Row(String),
    Hint(String),
}

/// Repos no rule matched, grouped by GitHub owner, when another profile has no
/// owners yet — the usual reason an office repo falls back to a personal default.
fn owner_summary(
    repos: &[ScannedRepo],
    statuses: &[RepoStatus],
    set: &ProfileSet,
) -> Vec<SummaryLine> {
    let candidates = ownerless_profiles(set);
    if candidates.is_empty() {
        return Vec::new();
    }
    // Lower-case key (GitHub owners are case-insensitive), first spelling kept.
    let mut groups: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for (r, s) in repos.iter().zip(statuses) {
        if matches!(
            s,
            RepoStatus::Error | RepoStatus::Pinned | RepoStatus::PinnedNow
        ) {
            continue;
        }
        if let Some(owner) = unmatched_owner(&r.resolution) {
            let e = groups
                .entry(owner.to_lowercase())
                .or_insert_with(|| (owner.to_lowercase(), 0));
            e.1 += 1;
        }
    }
    if groups.is_empty() {
        return Vec::new();
    }
    let total: usize = groups.values().map(|(_, n)| n).sum();
    let width = groups.values().map(|(o, _)| o.len()).max().unwrap_or(0);
    let mut out = vec![SummaryLine::Warn(format!(
        "{} matched no owner rule, so {} the default profile. By GitHub owner:",
        plural(total, "repo", "repos"),
        if total == 1 { "it uses" } else { "they use" }
    ))];
    for (owner, n) in groups.values() {
        out.push(SummaryLine::Row(format!(
            "    {owner:<width$}  {}",
            plural(*n, "repo", "repos")
        )));
    }
    let names = labels_and(&candidates);
    let has = if candidates.len() == 1 { "has" } else { "have" };
    let owners: Vec<&str> = groups.values().map(|(o, _)| o.as_str()).collect();
    match candidates.as_slice() {
        [c] => {
            out.push(SummaryLine::Hint(format!(
                "{names} {has} no owners yet. Add the ones that are {}'s:",
                c.label
            )));
            out.push(SummaryLine::Hint(format!(
                "  {}",
                add_owner_command(&candidates, &owners)
            )));
        }
        _ => {
            out.push(SummaryLine::Hint(format!(
                "{names} {has} no owners yet. Add each owner to its profile:"
            )));
            out.push(SummaryLine::Hint(format!(
                "  {}",
                add_owner_command(&candidates, &["<owner>"])
            )));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use githop_core::{EffectiveIdentity, Identity, Profile, ProfileColor, Resolution};

    fn set() -> ProfileSet {
        ProfileSet {
            version: 1,
            default: Some("office".into()),
            profiles: vec![Profile {
                id: "office".into(),
                label: "Office".into(),
                name: "Me".into(),
                email: "me@company.com".into(),
                color: ProfileColor::Blue,
                ssh_key: "C:/k".into(),
                owners: vec![],
                folders: vec![],
            }],
        }
    }

    fn repo(source: Option<ResolutionSource>, pinned: Option<&str>, email: &str) -> ScannedRepo {
        let ident = Identity {
            name: "Me".into(),
            email: email.into(),
        };
        ScannedRepo {
            root: "C:/work/api".into(),
            resolution: Resolution {
                profile_id: source.map(|_| "office".to_string()),
                source,
                remote_name: None,
                remote: None,
                pinned: pinned.map(str::to_string),
                warnings: vec![],
            },
            identity: Some(EffectiveIdentity {
                author: ident.clone(),
                committer: ident,
            }),
            error: None,
        }
    }

    #[test]
    fn classify_cases() {
        let s = set();
        assert_eq!(
            classify(
                &repo(
                    Some(ResolutionSource::Pin),
                    Some("office"),
                    "me@company.com"
                ),
                &s
            ),
            RepoStatus::Pinned
        );
        assert_eq!(
            classify(
                &repo(
                    Some(ResolutionSource::Owner),
                    Some("gone"),
                    "me@company.com"
                ),
                &s
            ),
            RepoStatus::StalePin
        );
        assert_eq!(
            classify(&repo(None, None, "x@y"), &s),
            RepoStatus::Unresolved
        );
        assert_eq!(
            classify(
                &repo(Some(ResolutionSource::Owner), None, "ME@company.com"),
                &s
            ),
            RepoStatus::NotPinned
        );
        assert_eq!(
            classify(
                &repo(Some(ResolutionSource::Folder), None, "home@me.dev"),
                &s
            ),
            RepoStatus::WrongIdentity
        );
        let mut e = repo(None, None, "x");
        e.error = Some("boom".into());
        assert_eq!(classify(&e, &s), RepoStatus::Error);
    }

    #[test]
    fn default_only_repos_need_flag() {
        let r = repo(Some(ResolutionSource::Default), None, "me@company.com");
        assert!(!pinnable(&r, RepoStatus::NotPinned, false));
        assert!(pinnable(&r, RepoStatus::NotPinned, true));
        let o = repo(Some(ResolutionSource::Owner), None, "me@company.com");
        assert!(pinnable(&o, RepoStatus::NotPinned, false));
        assert!(!pinnable(&o, RepoStatus::Pinned, false));
    }

    fn profile(id: &str, label: &str, owners: &[&str]) -> Profile {
        Profile {
            id: id.into(),
            label: label.into(),
            name: "Me".into(),
            email: format!("{id}@example.com"),
            color: ProfileColor::Blue,
            ssh_key: format!("C:/k/{id}").into(),
            owners: owners.iter().map(|o| o.to_string()).collect(),
            folders: vec![],
        }
    }

    /// The owner's real setup: Office has no owners, Personal is the default.
    fn office_unrouted() -> ProfileSet {
        ProfileSet {
            version: 1,
            default: Some("personal".into()),
            profiles: vec![
                profile("office", "Office", &[]),
                profile("personal", "Personal", &["harshaltupe12"]),
            ],
        }
    }

    fn default_repo(owner: &str) -> ScannedRepo {
        let mut r = repo(
            Some(ResolutionSource::Default),
            None,
            "personal@example.com",
        );
        r.resolution.profile_id = Some("personal".into());
        r.resolution.remote = Some(githop_core::RemoteInfo {
            host: "github.com".into(),
            owner: owner.into(),
            repo: "api".into(),
        });
        r
    }

    fn text(lines: &[SummaryLine]) -> Vec<String> {
        lines
            .iter()
            .map(|l| match l {
                SummaryLine::Warn(t) => format!("! {t}"),
                SummaryLine::Row(t) | SummaryLine::Hint(t) => t.clone(),
            })
            .collect()
    }

    #[test]
    fn summary_groups_default_only_repos_by_owner() {
        let set = office_unrouted();
        let mut pinned = default_repo("miraki-labs");
        pinned.resolution.source = Some(ResolutionSource::Pin);
        let repos = vec![
            default_repo("miraki-labs"),
            default_repo("Miraki-Labs"),
            default_repo("harshal-miraki"),
            pinned,
        ];
        let statuses: Vec<RepoStatus> = repos.iter().map(|r| classify(r, &set)).collect();
        assert_eq!(
            text(&owner_summary(&repos, &statuses, &set)),
            vec![
                "! 3 repos matched no owner rule, so they use the default profile. \
                 By GitHub owner:",
                "    harshal-miraki  1 repo",
                "    miraki-labs     2 repos",
                "Office has no owners yet. Add the ones that are Office's:",
                "  githop edit office --owner harshal-miraki --owner miraki-labs",
            ]
        );
    }

    #[test]
    fn summary_quiet_when_every_profile_has_owners() {
        let mut routed = office_unrouted();
        routed.profiles[0].owners = vec!["miraki-labs".into()];
        let repos = vec![default_repo("someone-else")];
        let statuses = vec![RepoStatus::NotPinned];
        assert!(owner_summary(&repos, &statuses, &routed).is_empty());
        // One profile: nothing to route to.
        assert!(owner_summary(&repos, &statuses, &set()).is_empty());
    }

    #[test]
    fn summary_with_several_ownerless_profiles_uses_placeholder() {
        let mut set = office_unrouted();
        set.profiles.push(profile("side", "Side", &[]));
        let repos = vec![default_repo("acme")];
        let statuses = vec![RepoStatus::NotPinned];
        let out = text(&owner_summary(&repos, &statuses, &set));
        assert_eq!(
            out[2..],
            [
                "Office and Side have no owners yet. Add each owner to its profile:",
                "  githop edit <profile> --owner <owner>  (office, side)",
            ]
        );
    }

    #[test]
    fn non_github_hosts_are_left_out() {
        let set = office_unrouted();
        let mut r = default_repo("team");
        r.resolution.warnings = vec![githop_core::ResolutionWarning::NonGithubHost {
            host: "gitlab.com".into(),
        }];
        assert!(owner_summary(&[r], &[RepoStatus::NotPinned], &set).is_empty());
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative("C:/Work/api", "c:/work/"), "api");
        assert_eq!(relative("C:/Work/a/b/", "C:/Work/"), "a/b");
        assert_eq!(relative("C:/Work", "C:/Work/"), ".");
        assert_eq!(relative("D:/x", "C:/Work/"), "D:/x");
    }
}
