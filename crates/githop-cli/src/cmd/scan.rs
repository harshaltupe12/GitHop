//! `githop scan <folder> [--apply] [--json]` — find repos and show which profile owns each.

use std::path::Path;

use anyhow::Result;
use githop_core::paths::normalize_repo_root;
use githop_core::scan::{self, ScannedRepo};
use githop_core::{pin as core_pin, ProfileSet, Repo, ResolutionSource};
use serde_json::json;

use super::{remember_repo, source_tag, warning_json, Ctx};
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
            ui.warn(&format!("{name}: {}", super::describe_warning(w, &set)));
        }
    }
    for (root, e) in &apply_errors {
        ui.warn(&format!("{}: couldn't pin ({e})", relative(root, &base)));
    }

    let count = |want: &[RepoStatus]| statuses.iter().filter(|s| want.contains(s)).count();
    ui.line(&format!(
        "{} repo(s): {} pinned, {} not pinned, {} unresolved.",
        repos.len(),
        count(&[RepoStatus::Pinned, RepoStatus::PinnedNow]),
        count(&[
            RepoStatus::NotPinned,
            RepoStatus::WrongIdentity,
            RepoStatus::StalePin
        ]),
        count(&[RepoStatus::Unresolved]),
    ));
    if args.apply {
        ui.ok(&format!("Pinned {} repo(s).", applied.len()));
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
        ui.hint(&format!(
            "{default_only} repo(s) only matched the default profile and weren't pinned. \
             Add owner or folder rules, or use --include-default."
        ));
    }
    Ok(Exit::Ok)
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

    #[test]
    fn relative_paths() {
        assert_eq!(relative("C:/Work/api", "c:/work/"), "api");
        assert_eq!(relative("C:/Work/a/b/", "C:/Work/"), "a/b");
        assert_eq!(relative("C:/Work", "C:/Work/"), ".");
        assert_eq!(relative("D:/x", "C:/Work/"), "D:/x");
    }
}
