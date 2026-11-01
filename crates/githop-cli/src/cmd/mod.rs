//! Command implementations and shared helpers.

pub mod add;
pub mod check;
pub mod doctor;
pub mod edit;
pub mod export;
pub mod fix;
pub mod hooks;
pub mod import;
pub mod init;
pub mod installer;
pub mod keygen;
pub mod list;
pub mod pin;
pub mod remote;
pub mod remove;
pub mod scan;
pub mod status;
pub mod test;
pub mod use_;
pub mod wipe;

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use anyhow::Result;
use githop_core::paths::{folder_key, normalize_repo_root, to_slash};
use githop_core::{
    backup, pin as core_pin, Git, Profile, ProfileSet, Repo, Resolution, ResolutionSource,
    ResolutionWarning, Store,
};

use crate::cli::Command;
use crate::exit::{usage, usage_hint, Exit};
use crate::ui::Ui;

/// Global git keys `githop use` writes (and `wipe` restores).
pub use githop_core::wipe::MANAGED_GLOBAL_KEYS;

/// Per-invocation context: output handle plus a lazily located `git`.
pub struct Ctx {
    pub ui: Ui,
    git: OnceCell<Git>,
}

impl Ctx {
    pub fn new(ui: Ui) -> Self {
        Self {
            ui,
            git: OnceCell::new(),
        }
    }

    /// `git`, located on PATH once per process.
    pub fn git(&self) -> Result<&Git> {
        if let Some(g) = self.git.get() {
            return Ok(g);
        }
        let g = Git::locate()?;
        Ok(self.git.get_or_init(|| g))
    }

    /// Open the GitHop store (`GITHOP_HOME` or `%APPDATA%\githop`).
    pub fn store(&self) -> Result<Store> {
        Ok(Store::open()?)
    }

    /// Commands without a JSON form refuse `--json` instead of mixing output.
    /// D1: refresh the SSH host alias cache (best effort, after a command
    /// that changed profiles).
    pub fn refresh_aliases(&self, paths: &githop_core::Paths) {
        if let Ok(git) = self.git() {
            githop_core::aliases::refresh_best_effort(paths, git);
        }
    }

    pub fn reject_json(&self, command: &str) -> Result<()> {
        if self.ui.json {
            return Err(usage_hint(
                format!("--json isn't available for githop {command}."),
                "JSON output: githop list, status, scan, doctor, test, check, fix, hooks status, wipe",
            ));
        }
        Ok(())
    }
}

/// Dispatch a parsed command.
pub fn run(ctx: &Ctx, command: Command) -> Result<Exit> {
    match command {
        Command::Init(a) => init::run(ctx, a),
        Command::Add(a) => add::run(ctx, a),
        Command::Edit(a) => edit::run(ctx, a),
        Command::Remove(a) => remove::run(ctx, a),
        Command::List => list::run(ctx),
        Command::Use(a) => use_::run(ctx, a),
        Command::Status => status::run(ctx),
        Command::Pin(a) => pin::run_pin(ctx, a),
        Command::Unpin => pin::run_unpin(ctx),
        Command::Scan(a) => scan::run(ctx, a),
        Command::Keygen(a) => keygen::run(ctx, a),
        Command::Test(a) => test::run(ctx, a),
        Command::Doctor => doctor::run(ctx),
        Command::Export(a) => export::run(ctx, a),
        Command::Import(a) => import::run(ctx, a),
        Command::Wipe(a) => wipe::run(ctx, a),
        Command::Check(a) => check::run(ctx, a),
        Command::Hooks(c) => hooks::run(ctx, c),
        Command::Remote(c) => remote::run(ctx, c),
        Command::Fix(a) => fix::run(ctx, a),
        Command::Installer(c) => installer::run(ctx, c),
    }
}

/// Current directory.
pub fn cwd() -> Result<PathBuf> {
    Ok(std::env::current_dir()?)
}

/// Look up a profile or fail with a usage error naming the known ids.
pub fn profile_or_usage<'a>(set: &'a ProfileSet, id: &str) -> Result<&'a Profile> {
    set.get(id).ok_or_else(|| {
        if set.profiles.is_empty() {
            usage_hint(
                format!("There's no profile called \"{id}\"."),
                "You have no profiles yet. Run: githop init",
            )
        } else {
            usage_hint(
                format!("There's no profile called \"{id}\"."),
                format!(
                    "Profiles: {}. Run: githop list",
                    profile_ids(set).join(", ")
                ),
            )
        }
    })
}

/// All profile ids, in file order.
pub fn profile_ids(set: &ProfileSet) -> Vec<String> {
    set.profiles.iter().map(|p| p.id.clone()).collect()
}

/// `Office (harshal@company.com)`.
pub fn profile_display(p: &Profile) -> String {
    format!("{} ({})", p.label, p.email)
}

/// Label for a profile id, or the id itself when the profile is gone.
pub fn label_for(set: &ProfileSet, id: &str) -> String {
    set.get(id)
        .map(|p| p.label.clone())
        .unwrap_or_else(|| id.to_string())
}

/// `office-laptop` → `Office laptop`.
pub fn label_from_id(id: &str) -> String {
    crate::exit::sentence(&id.replace(['-', '_'], " "))
}

/// `My Office!` → `my-office`.
pub fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(32).collect()
}

/// Split a comma/space separated answer into items (`-` = none).
pub fn split_list(answer: &str) -> Vec<String> {
    if answer.trim() == "-" {
        return Vec::new();
    }
    answer
        .split([',', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Does this look like a personal profile (for the noreply email tip)?
pub fn looks_personal(id: &str, label: &str) -> bool {
    let text = format!("{id} {label}").to_lowercase();
    ["personal", "home", "private", "side"]
        .iter()
        .any(|w| text.contains(w))
}

/// Tip shown before asking for a personal email (Design §9, Plan §9.3).
pub const NOREPLY_TIP: &str = "Tip: a GitHub noreply email keeps your address private, e.g. \
     ID+username@users.noreply.github.com (GitHub → Settings → Emails).";

/// Compare two key paths the way Windows does (slashes, case).
pub fn same_path(a: &str, b: &str) -> bool {
    a.trim()
        .replace('\\', "/")
        .eq_ignore_ascii_case(&b.trim().replace('\\', "/"))
}

/// Add a normalised repo root to `list` unless it's already there (case-insensitive).
pub fn remember_repo(list: &mut Vec<String>, root: &Path) -> bool {
    let norm = normalize_repo_root(root);
    let key = folder_key(&norm);
    if list.iter().any(|r| folder_key(r) == key) {
        return false;
    }
    list.push(norm);
    true
}

/// Remove a repo root from `list` (case-insensitive). Returns whether it was there.
pub fn forget_repo(list: &mut Vec<String>, root: &Path) -> bool {
    let key = folder_key(&normalize_repo_root(root));
    let before = list.len();
    list.retain(|r| folder_key(r) != key);
    list.len() != before
}

/// Human description of why a profile was chosen.
pub fn describe_source(res: &Resolution) -> String {
    match res.source {
        Some(ResolutionSource::Pin) => "pinned".to_string(),
        Some(ResolutionSource::Owner) => match (&res.remote, &res.remote_name) {
            (Some(r), Some(name)) => format!("owner {} on {name}", r.owner),
            (Some(r), None) => format!("owner {}", r.owner),
            _ => "owner rule".to_string(),
        },
        Some(ResolutionSource::Folder) => "folder rule".to_string(),
        Some(ResolutionSource::Default) => "default profile".to_string(),
        None => "no rule matched".to_string(),
    }
}

/// Non-default profiles with no owners yet. With two or more profiles an owner
/// rule never picks them, so their repos quietly use the default profile (how an
/// office repo got a personal commit: Office had no owners, Personal was default).
pub fn ownerless_profiles(set: &ProfileSet) -> Vec<&Profile> {
    if set.profiles.len() < 2 {
        return Vec::new();
    }
    let default = set.default.as_deref().map(str::trim);
    set.profiles
        .iter()
        .filter(|p| default != Some(p.id.trim()) && p.owners.iter().all(|o| o.trim().is_empty()))
        .collect()
}

/// The GitHub owner of a repo no rule matched (default profile or none, not pinned).
/// `None` for non-GitHub hosts, where owner rules don't apply.
pub fn unmatched_owner(res: &Resolution) -> Option<&str> {
    let no_rule = matches!(res.source, Some(ResolutionSource::Default) | None);
    let non_github = res
        .warnings
        .iter()
        .any(|w| matches!(w, ResolutionWarning::NonGithubHost { .. }));
    if !no_rule || res.pinned.is_some() || non_github {
        return None;
    }
    res.remote
        .as_ref()
        .map(|r| r.owner.as_str())
        .filter(|o| !o.trim().is_empty())
}

/// `githop edit office --owner miraki-labs`, or `<profile>` plus the choices.
/// `--owner` replaces a profile's owners, which is safe here: candidates have none.
pub fn add_owner_command(candidates: &[&Profile], owners: &[&str]) -> String {
    let flags = owners
        .iter()
        .map(|o| format!("--owner {o}"))
        .collect::<Vec<_>>()
        .join(" ");
    match candidates {
        [p] => format!("githop edit {} {flags}", p.id),
        _ => {
            let ids: Vec<&str> = candidates.iter().map(|p| p.id.as_str()).collect();
            format!("githop edit <profile> {flags}  ({})", ids.join(", "))
        }
    }
}

/// "Office" / "Office and Freelance" / "Office, Freelance and Side".
pub fn labels_and(profiles: &[&Profile]) -> String {
    let labels: Vec<&str> = profiles.iter().map(|p| p.label.as_str()).collect();
    match labels.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Short source tag for tables.
pub fn source_tag(res: &Resolution) -> &'static str {
    match res.source {
        Some(ResolutionSource::Pin) => "pin",
        Some(ResolutionSource::Owner) => "owner",
        Some(ResolutionSource::Folder) => "folder",
        Some(ResolutionSource::Default) => "default",
        None => "—",
    }
}

/// One plain-language sentence for a resolution warning (Design §9.1).
pub fn describe_warning(w: &ResolutionWarning, set: &ProfileSet) -> String {
    match w {
        ResolutionWarning::OwnerFolderDisagree {
            owner_profile,
            folder_profile,
        } => format!(
            "The owner rule says {} but the folder rule says {}. The owner rule wins.",
            label_for(set, owner_profile),
            label_for(set, folder_profile)
        ),
        ResolutionWarning::StalePin { profile_id } => format!(
            "This repo is pinned to \"{profile_id}\", which no longer exists. \
             Run: githop pin <profile>"
        ),
        ResolutionWarning::UnknownRemoteFormat { remote } => format!(
            "GitHop couldn't read the URL of remote \"{remote}\", so owner rules were skipped."
        ),
        ResolutionWarning::NonGithubHost { host } => {
            format!("Remote host {host} isn't GitHub, so owner rules were skipped.")
        }
        ResolutionWarning::LocalHooksPath { path } => format!(
            "This repo sets its own core.hooksPath ({path}), so GitHop's hooks don't run here. \
             Get the line to add to that hook: githop hooks snippet commit"
        ),
        ResolutionWarning::PinOwnerDisagree {
            pinned_profile,
            rule_profile,
            rule,
            owner,
        } => {
            let why = match (rule, owner) {
                (ResolutionSource::Owner, Some(o)) => format!("owner {o} belongs to"),
                (ResolutionSource::Folder, _) => "its folder belongs to".to_string(),
                _ => "its owner rule says".to_string(),
            };
            format!(
                "Pinned to {}, but {why} {}. The pin wins. Run: githop pin {rule_profile}",
                label_for(set, pinned_profile),
                label_for(set, rule_profile)
            )
        }
    }
}

/// Warning as JSON: its serde form plus a `message`.
pub fn warning_json(w: &ResolutionWarning, set: &ProfileSet) -> serde_json::Value {
    let mut v = serde_json::to_value(w).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(map) = &mut v {
        map.insert(
            "message".to_string(),
            serde_json::Value::String(describe_warning(w, set)),
        );
    }
    v
}

/// Key path as display text (forward slashes).
pub fn key_display(p: &Path) -> String {
    to_slash(p)
}

/// Outcome of [`switch_default`].
pub struct SwitchOutcome {
    /// `gitconfig.original` was created by this call.
    pub first_backup: bool,
}

/// Make `id` the default profile (Plan §7.6): back up the global config, record
/// the keys GitHop manages (saved *before* writing so `wipe` can always restore
/// them), set global `user.name`/`user.email`/`core.sshCommand`, then set
/// `default = id` (via `Store::update`, so concurrent edits aren't lost).
/// `profiles` is only used to look the profile up.
pub fn switch_default(
    ctx: &Ctx,
    store: &mut Store,
    profiles: ProfileSet,
    id: &str,
) -> Result<SwitchOutcome> {
    let git = ctx.git()?;
    let profile = profile_or_usage(&profiles, id)?.clone();
    pin::refuse_invalid(&profiles, &profile)?;
    let paths = store.paths().clone();
    paths.ensure_dirs()?;
    let first_backup = backup::original_gitconfig(&paths).is_none();
    let saved = backup::backup_global_gitconfig(&paths, git)?;
    ctx.ui.detail(&format!(
        "Global git config backed up to {}",
        to_slash(&saved)
    ));
    githop_core::aliases::refresh_best_effort(&paths, git);

    let missing = MANAGED_GLOBAL_KEYS.iter().any(|key| {
        !store
            .settings()
            .managed_global_keys
            .iter()
            .any(|k| k == key)
    });
    if missing {
        store.update_settings(|s| {
            for key in MANAGED_GLOBAL_KEYS {
                if !s.managed_global_keys.iter().any(|k| k == key) {
                    s.managed_global_keys.push(key.to_string());
                }
            }
            Ok(())
        })?;
    }

    core_pin::set_default_profile_global(git, &profile)?;

    store.update(|p, _| {
        p.default = Some(id.to_string());
        Ok(())
    })?;
    Ok(SwitchOutcome { first_backup })
}

/// Repos in `settings.pinned_repos` currently pinned to `id`.
pub fn repos_pinned_to(git: &Git, pinned_repos: &[String], id: &str) -> Vec<Repo> {
    pinned_repos
        .iter()
        .filter_map(|root| {
            let path = Path::new(root);
            if !path.exists() {
                return None;
            }
            let repo = Repo::discover(git, path).ok()?;
            let snap = repo.config_snapshot().ok()?;
            (snap.get_scoped(core_pin::PIN_KEY, githop_core::ConfigScope::Local) == Some(id))
                .then_some(repo)
        })
        .collect()
}

/// Fail with a usage error unless confirmation is possible or given.
pub fn require_confirmation(ctx: &Ctx, yes: bool, question: &str, flag_hint: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !ctx.ui.interactive {
        return Err(usage(format!(
            "This needs confirmation, and there's no terminal to ask in. {flag_hint}"
        )));
    }
    ctx.ui.confirm(question, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_label() {
        assert_eq!(slug("My Office!"), "my-office");
        assert_eq!(slug("  Personal  "), "personal");
        assert_eq!(label_from_id("office-laptop"), "Office laptop");
        assert_eq!(label_from_id("personal"), "Personal");
    }

    #[test]
    fn split_list_cases() {
        assert_eq!(split_list("a, b;c"), vec!["a", "b", "c"]);
        assert!(split_list("-").is_empty());
        assert!(split_list("  ").is_empty());
    }

    #[test]
    fn personal_detection() {
        assert!(looks_personal("personal", "Personal"));
        assert!(looks_personal("me", "Home stuff"));
        assert!(!looks_personal("office", "Office"));
    }

    #[test]
    fn same_path_ignores_slashes_and_case() {
        assert!(same_path(r"C:\Users\Me\.ssh\k", "c:/users/me/.ssh/k"));
        assert!(!same_path("C:/a", "C:/b"));
    }

    #[test]
    fn describe_source_variants() {
        let mut r = Resolution {
            profile_id: Some("office".into()),
            source: Some(ResolutionSource::Owner),
            remote_name: Some("origin".into()),
            remote: Some(githop_core::RemoteInfo {
                host: "github.com".into(),
                owner: "office-org".into(),
                repo: "api".into(),
            }),
            pinned: None,
            warnings: vec![],
        };
        assert_eq!(describe_source(&r), "owner office-org on origin");
        r.source = Some(ResolutionSource::Pin);
        assert_eq!(describe_source(&r), "pinned");
        r.source = None;
        assert_eq!(source_tag(&r), "—");
    }

    #[test]
    fn warnings_have_messages() {
        let set = ProfileSet::default();
        let w = ResolutionWarning::LocalHooksPath {
            path: ".husky".into(),
        };
        let v = warning_json(&w, &set);
        assert_eq!(v["kind"], "local-hooks-path");
        assert!(v["message"].as_str().unwrap().contains(".husky"));
    }
}
