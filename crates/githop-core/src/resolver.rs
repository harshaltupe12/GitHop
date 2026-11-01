//! Repo → profile resolution: pin → owner → folder → default (Plan §7.1, C7, C12, C13).
//!
//! CONTRACT FILE: types and signature fixed by the architect; body filled by the
//! Phase 1 "resolver" task. `resolve` must never spawn a process.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::gitconfig::{ConfigScope, ConfigSnapshot};
use crate::paths::{folder_key, normalize_folder, to_slash};
use crate::profile::ProfileSet;
use crate::remote::{parse_remote_url, RemoteInfo};

/// Which rule decided the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionSource {
    Pin,
    Owner,
    Folder,
    Default,
}

/// Non-fatal findings during resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResolutionWarning {
    /// Owner rule and folder rule point at different profiles; owner won (C7).
    OwnerFolderDisagree {
        owner_profile: String,
        folder_profile: String,
    },
    /// `githop.profile` names a profile that no longer exists.
    StalePin { profile_id: String },
    /// The chosen remote's URL couldn't be parsed.
    UnknownRemoteFormat { remote: String },
    /// Remote host isn't GitHub; owner rules skipped (v1).
    NonGithubHost { host: String },
    /// Repo sets `core.hooksPath` locally (husky etc.) — GitHop hooks won't run (C3).
    LocalHooksPath { path: String },
    /// The pin decided, but the repo's owner or folder rule now names another
    /// profile (regression D7): e.g. pinned to Personal before `acme-corp` was
    /// added to Office's owners. The pin still wins; this says so.
    PinOwnerDisagree {
        /// Profile the repo is pinned to.
        pinned_profile: String,
        /// Profile the owner (or folder) rule picks.
        rule_profile: String,
        /// `owner` or `folder`.
        rule: ResolutionSource,
        /// The remote's owner, for an owner rule.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
    },
}

/// Result of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// `None` = unresolved.
    pub profile_id: Option<String>,
    pub source: Option<ResolutionSource>,
    /// Remote name used for owner matching, if any.
    pub remote_name: Option<String>,
    /// Parsed remote, if any.
    pub remote: Option<RemoteInfo>,
    /// Value of `githop.profile` (even if stale).
    pub pinned: Option<String>,
    pub warnings: Vec<ResolutionWarning>,
}

/// Resolve which profile owns the repo at `repo_root`.
///
/// 1. Pin (`githop.profile`, local scope) if it names an existing profile; stale → warning, continue.
/// 2. Owner: remote = `githop.remote` if set, else `origin`, else first remote alphabetically;
///    only when `RemoteInfo::is_github(github_aliases)`.
/// 3. Folder: longest matching normalised prefix, case-insensitive (`paths::folder_key`).
/// 4. `set.default` if it exists.
///
/// Owner and folder disagree → owner wins + `OwnerFolderDisagree`. Local
/// `core.hooksPath` → `LocalHooksPath` warning. No process spawns.
///
/// Details:
/// - `remote_name` / `remote` are always filled when the repo has a remote, even
///   when a pin decides; the remote warnings (`UnknownRemoteFormat`,
///   `NonGithubHost`) are only emitted when the owner rule was actually consulted.
/// - A `githop.remote` naming a remote without a URL falls back to `origin` / first.
/// - The remote's URL is its **first** `remote.<name>.url` (git fetch semantics).
///   When that URL as written isn't a GitHub URL, `url.<base>.insteadOf` rules
///   from the snapshot are applied (longest prefix wins, like git) and the
///   rewritten URL is used if it parses — so `gh:acme/x` shorthands resolve,
///   while a GitHub URL redirected to a mirror/local path keeps its owner.
/// - Folder rules (review #5): each rule and `repo_root` are compared both as
///   written (normalised) and canonicalised through the file system when the
///   path exists, so a rule on a junction, `subst` drive or mapped drive matches
///   the resolved path git reports, and vice versa. Decision: canonicalise at
///   match time (a few `stat`-like calls, no spawns) rather than storing a
///   second canonical copy in `profiles.toml` — the file format stays unchanged
///   and a rule created before its folder existed still starts matching later.
pub fn resolve(
    snapshot: &ConfigSnapshot,
    repo_root: &Path,
    set: &ProfileSet,
    github_aliases: &[String],
) -> Resolution {
    let mut warnings = Vec::new();

    // C3: a repo-local hooksPath means the global GitHop hooks never run here.
    if let Some(path) = local_value(snapshot, "core.hooksPath") {
        warnings.push(ResolutionWarning::LocalHooksPath {
            path: path.to_string(),
        });
    }

    let (remote_name, remote_url) = pick_remote(snapshot);
    let remote = remote_url.and_then(|url| parse_remote(snapshot, url, github_aliases));

    let mut res = Resolution {
        profile_id: None,
        source: None,
        remote_name: remote_name.clone(),
        remote: remote.clone(),
        pinned: None,
        warnings: Vec::new(),
    };

    // 1. Pin.
    let pinned = snapshot
        .get_scoped("githop.profile", ConfigScope::Local)
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if let Some(pin) = pinned {
        res.pinned = Some(pin.to_string());
        if set.get(pin).is_some() {
            res.profile_id = Some(pin.to_string());
            res.source = Some(ResolutionSource::Pin);
            // D7: say when the owner/folder rule would now pick another profile.
            let owner = remote
                .as_ref()
                .filter(|i| i.is_github(github_aliases))
                .and_then(|i| owner_match(set, &i.owner).map(|p| (p, i.owner.clone())));
            let rule = match owner {
                Some((p, o)) => Some((p, ResolutionSource::Owner, Some(o))),
                None => folder_match(repo_root, set).map(|p| (p, ResolutionSource::Folder, None)),
            };
            if let Some((rule_profile, rule, owner)) = rule.filter(|(p, _, _)| *p != pin) {
                warnings.push(ResolutionWarning::PinOwnerDisagree {
                    pinned_profile: pin.to_string(),
                    rule_profile: rule_profile.to_string(),
                    rule,
                    owner,
                });
            }
            res.warnings = warnings;
            return res;
        }
        warnings.push(ResolutionWarning::StalePin {
            profile_id: pin.to_string(),
        });
    }

    // 2. Owner.
    let owner_profile = match (&remote_name, remote_url, &remote) {
        (Some(name), Some(_), None) => {
            warnings.push(ResolutionWarning::UnknownRemoteFormat {
                remote: name.clone(),
            });
            None
        }
        (_, _, Some(info)) if !info.is_github(github_aliases) => {
            warnings.push(ResolutionWarning::NonGithubHost {
                host: info.host.clone(),
            });
            None
        }
        (_, _, Some(info)) => owner_match(set, &info.owner),
        _ => None,
    };

    // 3. Folder.
    let folder_profile = folder_match(repo_root, set);

    let decided = match (owner_profile, folder_profile) {
        (Some(owner), folder) => {
            if let Some(folder) = folder.filter(|f| *f != owner) {
                warnings.push(ResolutionWarning::OwnerFolderDisagree {
                    owner_profile: owner.to_string(),
                    folder_profile: folder.to_string(),
                });
            }
            Some((owner, ResolutionSource::Owner))
        }
        (None, Some(folder)) => Some((folder, ResolutionSource::Folder)),
        // 4. Default.
        (None, None) => set
            .default_profile()
            .map(|p| (p.id.as_str(), ResolutionSource::Default)),
    };

    if let Some((id, source)) = decided {
        res.profile_id = Some(id.to_string());
        res.source = Some(source);
    }
    res.warnings = warnings;
    res
}

/// Profile whose owner rules list `owner` (case-insensitive).
fn owner_match<'a>(set: &'a ProfileSet, owner: &str) -> Option<&'a str> {
    set.profiles
        .iter()
        .find(|p| {
            p.owners
                .iter()
                .any(|o| o.trim().eq_ignore_ascii_case(owner))
        })
        .map(|p| p.id.as_str())
}

/// Non-empty value of `key` set in the repo itself (worktree beats local).
fn local_value<'a>(snapshot: &'a ConfigSnapshot, key: &str) -> Option<&'a str> {
    snapshot
        .get_scoped(key, ConfigScope::Worktree)
        .or_else(|| snapshot.get_scoped(key, ConfigScope::Local))
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Remote used for owner matching (C13): `githop.remote` → `origin` → first sorted.
fn pick_remote(snapshot: &ConfigSnapshot) -> (Option<String>, Option<&str>) {
    let configured = snapshot
        .get("githop.remote")
        .map(str::trim)
        .filter(|v| !v.is_empty());
    for name in configured.into_iter().chain(std::iter::once("origin")) {
        if let Some(url) = snapshot.remote_url(name) {
            return (Some(name.to_string()), Some(url));
        }
    }
    match snapshot.remotes().into_iter().next() {
        Some(name) => {
            let url = snapshot.remote_url(&name);
            (Some(name), url)
        }
        None => (None, None),
    }
}

/// Parse the remote URL; apply `insteadOf` when the URL as written isn't GitHub.
fn parse_remote(snapshot: &ConfigSnapshot, url: &str, aliases: &[String]) -> Option<RemoteInfo> {
    let raw = parse_remote_url(url);
    if raw.as_ref().is_some_and(|i| i.is_github(aliases)) {
        return raw;
    }
    snapshot
        .rewrite_url(url)
        .and_then(|u| parse_remote_url(&u))
        .or(raw)
}

/// Longest folder rule that contains `repo_root` (C6). Ties → first profile.
/// Rules and root are tried as written and canonicalised (see [`resolve`]).
fn folder_match<'a>(repo_root: &Path, set: &'a ProfileSet) -> Option<&'a str> {
    let root_keys = path_keys(&normalize_root(repo_root));
    let mut best: Option<(usize, &str)> = None;
    for profile in &set.profiles {
        for folder in &profile.folders {
            let Some(norm) = normalize_folder(folder) else {
                continue;
            };
            for key in path_keys(&norm) {
                if root_keys.iter().any(|r| r.starts_with(&key))
                    && best.is_none_or(|(len, _)| key.len() > len)
                {
                    best = Some((key.len(), profile.id.as_str()));
                }
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Comparison keys for a normalised absolute path: as written, plus the
/// canonical (junctions/subst/mapped drives resolved) form when the path
/// exists and differs. File system only, never a spawn.
fn path_keys(normalized: &str) -> Vec<String> {
    let mut keys = vec![folder_key(normalized)];
    if !crate::paths::is_absolute_str(normalized) {
        return keys;
    }
    if let Ok(canon) = dunce::canonicalize(Path::new(normalized)) {
        let key = folder_key(&normalize_root(&canon));
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Pure repo-root normalisation: forward slashes, `\\?\` stripped, duplicate
/// separators collapsed, trailing `/`. No canonicalisation (callers pass
/// `Repo::root()`, which is already absolute).
fn normalize_root(repo_root: &Path) -> String {
    let s = to_slash(repo_root);
    let s = if let Some(rest) = s.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = s.strip_prefix("//?/") {
        rest.to_string()
    } else {
        s
    };
    normalize_folder(&s).unwrap_or_else(|| if s.ends_with('/') { s } else { format!("{s}/") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitconfig::ConfigEntry;
    use crate::profile::{Profile, ProfileColor};
    use std::path::PathBuf;

    fn entry(scope: ConfigScope, key: &str, value: &str) -> ConfigEntry {
        ConfigEntry {
            scope,
            origin: String::new(),
            key: key.into(),
            value: Some(value.into()),
        }
    }

    fn local(key: &str, value: &str) -> ConfigEntry {
        entry(ConfigScope::Local, key, value)
    }

    fn snap(entries: Vec<ConfigEntry>) -> ConfigSnapshot {
        ConfigSnapshot::from_entries(entries)
    }

    fn origin(url: &str) -> ConfigEntry {
        local("remote.origin.url", url)
    }

    fn profile(id: &str, owners: &[&str], folders: &[&str]) -> Profile {
        Profile {
            id: id.into(),
            label: id.into(),
            name: id.into(),
            email: format!("{id}@example.com"),
            color: ProfileColor::Blue,
            ssh_key: PathBuf::from(format!("C:/keys/{id}")),
            owners: owners.iter().map(|s| s.to_string()).collect(),
            folders: folders.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// office: owner acme, folder C:/Office/ · personal: owner harshal, folder C:/Personal/
    fn two() -> ProfileSet {
        ProfileSet {
            version: 1,
            default: Some("personal".into()),
            profiles: vec![
                profile("office", &["acme"], &["C:/Office/"]),
                profile("personal", &["harshal"], &["C:/Personal/"]),
            ],
        }
    }

    fn no_default() -> ProfileSet {
        ProfileSet {
            default: None,
            ..two()
        }
    }

    fn id(r: &Resolution) -> Option<&str> {
        r.profile_id.as_deref()
    }

    #[test]
    fn precedence_table() {
        let set = two();
        let nd = no_default();
        struct Case {
            name: &'static str,
            entries: Vec<ConfigEntry>,
            root: &'static str,
            set: &'static str,
            want: Option<&'static str>,
            source: Option<ResolutionSource>,
        }
        let cases = vec![
            Case {
                name: "pin beats owner and folder",
                entries: vec![
                    origin("git@github.com:acme/x.git"),
                    local("githop.profile", "personal"),
                ],
                root: "C:/Office/x",
                set: "two",
                want: Some("personal"),
                source: Some(ResolutionSource::Pin),
            },
            Case {
                name: "owner beats folder and default",
                entries: vec![origin("https://github.com/ACME/x")],
                root: "C:/Personal/x",
                set: "two",
                want: Some("office"),
                source: Some(ResolutionSource::Owner),
            },
            Case {
                name: "folder when owner unknown",
                entries: vec![origin("git@github.com:stranger/x.git")],
                root: "C:/Office/x",
                set: "two",
                want: Some("office"),
                source: Some(ResolutionSource::Folder),
            },
            Case {
                name: "folder when no remote (C12)",
                entries: vec![],
                root: r"C:\Office\new",
                set: "two",
                want: Some("office"),
                source: Some(ResolutionSource::Folder),
            },
            Case {
                name: "default when nothing matches",
                entries: vec![origin("git@github.com:stranger/x.git")],
                root: "D:/elsewhere",
                set: "two",
                want: Some("personal"),
                source: Some(ResolutionSource::Default),
            },
            Case {
                name: "unresolved without default",
                entries: vec![],
                root: "D:/elsewhere",
                set: "nd",
                want: None,
                source: None,
            },
        ];
        for c in cases {
            let s = if c.set == "two" { &set } else { &nd };
            let r = resolve(&snap(c.entries), Path::new(c.root), s, &[]);
            assert_eq!(id(&r), c.want, "{}", c.name);
            assert_eq!(r.source, c.source, "{}", c.name);
        }
    }

    #[test]
    fn pin_fills_remote_fields_without_remote_warnings() {
        let s = snap(vec![
            origin("https://gitlab.com/acme/x"),
            local("githop.profile", "office"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.source, Some(ResolutionSource::Pin));
        assert_eq!(r.pinned.as_deref(), Some("office"));
        assert_eq!(r.remote_name.as_deref(), Some("origin"));
        assert_eq!(
            r.remote.as_ref().map(|i| i.host.as_str()),
            Some("gitlab.com")
        );
        assert!(r.warnings.is_empty());
    }

    /// Regression D7: a pin that the owner/folder rule disagrees with is reported.
    #[test]
    fn pin_owner_disagreement_is_reported() {
        let s = snap(vec![
            origin("https://github.com/acme/svc.git"),
            local("githop.profile", "personal"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.source, Some(ResolutionSource::Pin));
        assert_eq!(id(&r), Some("personal"));
        assert_eq!(
            r.warnings,
            vec![ResolutionWarning::PinOwnerDisagree {
                pinned_profile: "personal".into(),
                rule_profile: "office".into(),
                rule: ResolutionSource::Owner,
                owner: Some("acme".into()),
            }]
        );
        // Folder rule disagreeing.
        let s = snap(vec![local("githop.profile", "personal")]);
        let r = resolve(&s, Path::new("C:/Office/x"), &two(), &[]);
        assert!(matches!(
            r.warnings.as_slice(),
            [ResolutionWarning::PinOwnerDisagree {
                rule: ResolutionSource::Folder,
                owner: None,
                ..
            }]
        ));
        // Agreement: no warning.
        let s = snap(vec![
            origin("https://github.com/acme/svc.git"),
            local("githop.profile", "office"),
        ]);
        assert!(resolve(&s, Path::new("D:/x"), &two(), &[])
            .warnings
            .is_empty());
    }

    #[test]
    fn pin_must_be_local_scope() {
        let s = snap(vec![entry(ConfigScope::Global, "githop.profile", "office")]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.pinned, None);
        assert_eq!(r.source, Some(ResolutionSource::Default));
    }

    #[test]
    fn empty_pin_is_ignored() {
        let s = snap(vec![local("githop.profile", "  ")]);
        let r = resolve(&s, Path::new("C:/Office/x"), &two(), &[]);
        assert_eq!(r.pinned, None);
        assert_eq!(r.source, Some(ResolutionSource::Folder));
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn stale_pin_warns_and_continues() {
        let s = snap(vec![
            local("githop.profile", "deleted"),
            origin("git@github.com:harshal/x.git"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.pinned.as_deref(), Some("deleted"));
        assert_eq!(id(&r), Some("personal"));
        assert_eq!(r.source, Some(ResolutionSource::Owner));
        assert_eq!(
            r.warnings,
            vec![ResolutionWarning::StalePin {
                profile_id: "deleted".into()
            }]
        );
    }

    #[test]
    fn owner_folder_disagree_owner_wins() {
        let s = snap(vec![origin("git@github.com:harshal/x.git")]);
        let r = resolve(&s, Path::new("C:/Office/x"), &two(), &[]);
        assert_eq!(id(&r), Some("personal"));
        assert_eq!(r.source, Some(ResolutionSource::Owner));
        assert_eq!(
            r.warnings,
            vec![ResolutionWarning::OwnerFolderDisagree {
                owner_profile: "personal".into(),
                folder_profile: "office".into(),
            }]
        );
    }

    #[test]
    fn owner_and_folder_agree_no_warning() {
        let s = snap(vec![origin("git@github.com:acme/x.git")]);
        let r = resolve(&s, Path::new("C:/Office/x"), &two(), &[]);
        assert_eq!(r.source, Some(ResolutionSource::Owner));
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn overlapping_folders_longest_prefix_wins() {
        let set = ProfileSet {
            version: 1,
            default: None,
            profiles: vec![
                profile("work", &[], &["C:/Work/"]),
                profile("side", &[], &["C:/Work/Side/"]),
                profile("deep", &[], &["C:/Work/Side/Deep/x/"]),
            ],
        };
        let at = |p: &str| id(&resolve(&snap(vec![]), Path::new(p), &set, &[])).map(String::from);
        assert_eq!(at("C:/Work/a").as_deref(), Some("work"));
        assert_eq!(at("C:/Work/Side/a").as_deref(), Some("side"));
        assert_eq!(at("C:/Work/Side").as_deref(), Some("side"));
        assert_eq!(at("C:/Work/Side/Deep/y").as_deref(), Some("side"));
        assert_eq!(at("C:/Workshop/a"), None);
        assert_eq!(at("C:/Wor"), None);
    }

    #[test]
    fn folder_rules_are_normalised_and_case_insensitive() {
        let set = ProfileSet {
            version: 1,
            default: None,
            profiles: vec![
                profile("bad", &[], &["relative/dir", ""]),
                profile("office", &[], &[r"C:\Office"]),
                profile("share", &[], &["//Server/Share/Repos/"]),
            ],
        };
        let at = |p: &str| id(&resolve(&snap(vec![]), Path::new(p), &set, &[])).map(String::from);
        assert_eq!(at("c:/office/x").as_deref(), Some("office"));
        assert_eq!(at(r"C:\OFFICE\x").as_deref(), Some("office"));
        assert_eq!(at(r"C:\\Office\\\x\").as_deref(), Some("office"));
        assert_eq!(at(r"\\?\C:\Office\x").as_deref(), Some("office"));
        assert_eq!(at(r"\\server\share\repos\x").as_deref(), Some("share"));
        assert_eq!(
            at(r"\\?\UNC\server\share\repos\x").as_deref(),
            Some("share")
        );
        assert_eq!(at("//server/share/other"), None);
        assert_eq!(at("relative/dir/x"), None);
    }

    #[test]
    fn normalize_root_cases() {
        assert_eq!(normalize_root(Path::new(r"C:\a\b")), "C:/a/b/");
        assert_eq!(normalize_root(Path::new("C:/a/b/")), "C:/a/b/");
        assert_eq!(normalize_root(Path::new(r"\\?\C:\a")), "C:/a/");
        assert_eq!(
            normalize_root(Path::new(r"\\?\UNC\srv\sh\a")),
            "//srv/sh/a/"
        );
        assert_eq!(normalize_root(Path::new(r"\\srv\sh\a")), "//srv/sh/a/");
        assert_eq!(normalize_root(Path::new("rel/x")), "rel/x/");
        assert_eq!(normalize_root(Path::new("rel/x/")), "rel/x/");
    }

    #[test]
    fn githop_remote_override() {
        let s = snap(vec![
            origin("git@github.com:harshal/fork.git"),
            local("remote.upstream.url", "git@github.com:acme/x.git"),
            local("githop.remote", "upstream"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.remote_name.as_deref(), Some("upstream"));
        assert_eq!(id(&r), Some("office"));
        assert_eq!(r.source, Some(ResolutionSource::Owner));
    }

    #[test]
    fn githop_remote_missing_falls_back_to_origin() {
        let s = snap(vec![
            origin("git@github.com:harshal/fork.git"),
            local("githop.remote", "gone"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.remote_name.as_deref(), Some("origin"));
        assert_eq!(id(&r), Some("personal"));
    }

    #[test]
    fn origin_preferred_over_other_remotes() {
        let s = snap(vec![
            local("remote.aaa.url", "git@github.com:acme/x.git"),
            origin("git@github.com:harshal/x.git"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.remote_name.as_deref(), Some("origin"));
        assert_eq!(id(&r), Some("personal"));
    }

    #[test]
    fn origin_missing_uses_alphabetical_first() {
        let s = snap(vec![
            local("remote.zeta.url", "git@github.com:harshal/x.git"),
            local("remote.beta.url", "git@github.com:acme/x.git"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.remote_name.as_deref(), Some("beta"));
        assert_eq!(id(&r), Some("office"));
        assert_eq!(r.remote.as_ref().map(|i| i.owner.as_str()), Some("acme"));
    }

    #[test]
    fn non_github_host_skips_owner() {
        let s = snap(vec![origin("git@gitlab.com:acme/x.git")]);
        let r = resolve(&s, Path::new("C:/Personal/x"), &two(), &[]);
        assert_eq!(id(&r), Some("personal"));
        assert_eq!(r.source, Some(ResolutionSource::Folder));
        assert_eq!(
            r.warnings,
            vec![ResolutionWarning::NonGithubHost {
                host: "gitlab.com".into()
            }]
        );
    }

    #[test]
    fn uncached_alias_is_non_github_cached_alias_is_github() {
        let s = snap(vec![origin("git@github-office:ACME/x.git")]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.source, Some(ResolutionSource::Default));
        assert_eq!(
            r.warnings,
            vec![ResolutionWarning::NonGithubHost {
                host: "github-office".into()
            }]
        );
        let aliases = vec!["GitHub-Office".to_string()];
        let r = resolve(&s, Path::new("D:/x"), &two(), &aliases);
        assert_eq!(id(&r), Some("office"));
        assert_eq!(r.source, Some(ResolutionSource::Owner));
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn unknown_remote_format_warns() {
        let s = snap(vec![origin("C:/repos/bare.git")]);
        let r = resolve(&s, Path::new("C:/Office/x"), &two(), &[]);
        assert_eq!(r.remote_name.as_deref(), Some("origin"));
        assert_eq!(r.remote, None);
        assert_eq!(r.source, Some(ResolutionSource::Folder));
        assert_eq!(
            r.warnings,
            vec![ResolutionWarning::UnknownRemoteFormat {
                remote: "origin".into()
            }]
        );
    }

    #[test]
    fn owner_match_is_case_insensitive_and_trimmed() {
        let set = ProfileSet {
            version: 1,
            default: None,
            profiles: vec![profile("office", &[" AcMe "], &[])],
        };
        let s = snap(vec![origin("https://github.com/acme/x")]);
        let r = resolve(&s, Path::new("D:/x"), &set, &[]);
        assert_eq!(id(&r), Some("office"));
    }

    #[test]
    fn stale_default_is_unresolved() {
        let set = ProfileSet {
            default: Some("gone".into()),
            ..two()
        };
        let r = resolve(&snap(vec![]), Path::new("D:/x"), &set, &[]);
        assert_eq!(r.profile_id, None);
        assert_eq!(r.source, None);
    }

    #[test]
    fn local_hooks_path_warns() {
        let s = snap(vec![
            entry(ConfigScope::Global, "core.hooksPath", "C:/githop/hooks"),
            local("core.hookspath", ".husky"),
        ]);
        let r = resolve(&s, Path::new("C:/Office/x"), &two(), &[]);
        assert_eq!(
            r.warnings,
            vec![ResolutionWarning::LocalHooksPath {
                path: ".husky".into()
            }]
        );
        // Global-only hooksPath (GitHop's own) is fine.
        let s = snap(vec![entry(
            ConfigScope::Global,
            "core.hooksPath",
            "C:/githop/hooks",
        )]);
        assert!(resolve(&s, Path::new("C:/x"), &two(), &[])
            .warnings
            .is_empty());
        // Worktree scope counts as repo-local.
        let s = snap(vec![entry(
            ConfigScope::Worktree,
            "core.hooksPath",
            ".lefthook",
        )]);
        assert_eq!(
            resolve(&s, Path::new("C:/x"), &two(), &[]).warnings,
            vec![ResolutionWarning::LocalHooksPath {
                path: ".lefthook".into()
            }]
        );
    }

    #[test]
    fn warnings_accumulate_in_order() {
        let s = snap(vec![
            local("core.hooksPath", ".husky"),
            local("githop.profile", "gone"),
            origin("git@github.com:harshal/x.git"),
        ]);
        let r = resolve(&s, Path::new("C:/Office/x"), &two(), &[]);
        assert_eq!(id(&r), Some("personal"));
        assert_eq!(r.warnings.len(), 3);
        assert!(matches!(
            r.warnings[0],
            ResolutionWarning::LocalHooksPath { .. }
        ));
        assert!(matches!(r.warnings[1], ResolutionWarning::StalePin { .. }));
        assert!(matches!(
            r.warnings[2],
            ResolutionWarning::OwnerFolderDisagree { .. }
        ));
    }

    /// Review #7: git fetches from the first `remote.origin.url`.
    #[test]
    fn first_remote_url_decides_owner() {
        let s = snap(vec![
            origin("git@github.com:acme/x.git"),
            origin("git@github.com:harshal/x.git"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(id(&r), Some("office"));
        assert_eq!(r.remote.as_ref().map(|i| i.owner.as_str()), Some("acme"));
    }

    /// Review #7: `url.<base>.insteadOf` shorthands resolve to their owner.
    #[test]
    fn insteadof_shorthand_is_rewritten() {
        let s = snap(vec![
            entry(ConfigScope::Global, "url.git@github.com:.insteadof", "gh:"),
            origin("gh:ACME/x"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(id(&r), Some("office"));
        assert_eq!(r.source, Some(ResolutionSource::Owner));
        assert_eq!(
            r.remote.as_ref().map(|i| i.host.as_str()),
            Some("github.com")
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);

        // Without the rule the shorthand is a non-GitHub host "gh".
        let s = snap(vec![origin("gh:ACME/x")]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(r.source, Some(ResolutionSource::Default));

        // A GitHub URL redirected elsewhere (mirror, local path) keeps its owner.
        let s = snap(vec![
            entry(
                ConfigScope::Global,
                "url.C:/mirror/.insteadOf",
                "https://github.com/",
            ),
            origin("https://github.com/harshal/x.git"),
        ]);
        let r = resolve(&s, Path::new("D:/x"), &two(), &[]);
        assert_eq!(id(&r), Some("personal"));
        assert_eq!(r.source, Some(ResolutionSource::Owner));
    }

    /// Review #5: a folder rule on a junction matches the resolved path git
    /// reports, and a rule on the real folder matches a root given via the junction.
    #[cfg(windows)]
    #[test]
    fn folder_rules_match_through_junctions() {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let real = base.join("real work");
        let repo = real.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let link = base.join("jwork");
        let ok = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&real)
            .stdout(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("skipping: couldn't create a junction");
            return;
        }
        let slash = |p: &Path| to_slash(p);

        // Rule written on the junction; git reports the resolved root.
        let set = ProfileSet {
            version: 1,
            default: None,
            profiles: vec![profile("office", &[], &[&slash(&link)])],
        };
        let r = resolve(&snap(vec![]), &repo, &set, &[]);
        assert_eq!(id(&r), Some("office"));
        assert_eq!(r.source, Some(ResolutionSource::Folder));

        // Rule on the real folder; root given through the junction.
        let set = ProfileSet {
            version: 1,
            default: None,
            profiles: vec![profile("office", &[], &[&slash(&real)])],
        };
        let r = resolve(&snap(vec![]), &link.join("repo"), &set, &[]);
        assert_eq!(id(&r), Some("office"));

        // A sibling folder still doesn't match.
        let set = ProfileSet {
            version: 1,
            default: None,
            profiles: vec![profile("office", &[], &[&slash(&base.join("real"))])],
        };
        assert_eq!(id(&resolve(&snap(vec![]), &repo, &set, &[])), None);
    }

    #[test]
    fn serde_shape_of_warning() {
        let w = ResolutionWarning::StalePin {
            profile_id: "x".into(),
        };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("kind = \"stale-pin\""), "{s}");
    }

    /// `cargo test -p githop-core --lib resolver::tests::bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_resolve_10k() {
        let mut set = two();
        for i in 0..20 {
            set.profiles.push(profile(
                &format!("p{i}"),
                &[&format!("org{i}")],
                &[&format!("D:/Clients/Client{i}/")],
            ));
        }
        let mut entries: Vec<ConfigEntry> = (0..150)
            .map(|i| entry(ConfigScope::Global, &format!("alias.a{i}"), "log"))
            .collect();
        entries.push(origin("git@github.com:org19/repo.git"));
        entries.push(local("remote.upstream.url", "https://github.com/acme/repo"));
        entries.push(local("core.hooksPath", ".husky"));
        let s = snap(entries);
        let root = Path::new(r"D:\Clients\Client7\repo");
        let n = 10_000u32;
        let start = std::time::Instant::now();
        for _ in 0..n {
            let r = resolve(std::hint::black_box(&s), root, &set, &[]);
            std::hint::black_box(r);
        }
        let avg = start.elapsed() / n;
        println!("resolve avg: {avg:?}");
        assert!(avg < std::time::Duration::from_millis(5));
    }
}
