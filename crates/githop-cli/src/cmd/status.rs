//! `githop status [--json]` — which identity will this repo use, and why.

use anyhow::Result;
use githop_core::gitconfig::ConfigScope;
use githop_core::hooks;
use githop_core::paths::to_slash;
use githop_core::text::wrap;
use githop_core::{
    pin as core_pin, resolve, EffectiveIdentity, Identity, Profile, ProfileSet, Repo, Resolution,
    ResolutionSource,
};
use serde_json::json;

use super::{
    add_owner_command, cwd, describe_source, describe_warning, key_display, ownerless_profiles,
    profile_ids, same_path, unmatched_owner, warning_json, Ctx,
};
use crate::exit::Exit;

/// Pad labels so values line up.
fn row(label: &str, value: &str) -> String {
    format!("{label:<10}{value}")
}

fn ident_text(i: &Identity) -> String {
    format!("{} <{}>", i.name, i.email)
}

fn identity_matches(id: &EffectiveIdentity, p: &Profile) -> bool {
    id.author.email_matches(&p.email) && id.committer.email_matches(&p.email)
}

/// Warning + command when this repo's GitHub owner matched no rule and another
/// profile has no owners yet (Design §9.1: what happened, why, one next step).
fn owner_hint_lines(
    set: &ProfileSet,
    res: &Resolution,
    profile: Option<&Profile>,
) -> Option<(String, String)> {
    let owner = unmatched_owner(res)?;
    let candidates = ownerless_profiles(set);
    if candidates.is_empty() {
        return None;
    }
    let warn = match profile {
        Some(p) => format!(
            "No profile owns {owner} on GitHub, so this repo uses your default profile ({}).",
            p.label
        ),
        None => format!("No profile owns {owner} on GitHub, and there's no default profile."),
    };
    let cmd = add_owner_command(&candidates, &[owner]);
    let hint = match candidates.as_slice() {
        [c] => format!("If it's {} repo, run: {cmd}", article(&c.label)),
        _ => format!("Add it to the profile it belongs to: {cmd}"),
    };
    Some((warn, hint))
}

/// "an Office" / "a Personal".
fn article(label: &str) -> String {
    let vowel = label
        .chars()
        .next()
        .is_some_and(|c| "aeiouAEIOU".contains(c));
    format!("{} {label}", if vowel { "an" } else { "a" })
}

pub fn run(ctx: &Ctx) -> Result<Exit> {
    let git = ctx.git()?;
    let repo = Repo::discover(git, &cwd()?)?;
    let store = ctx.store()?;
    let set = store.profiles();
    let snapshot = repo.config_snapshot()?;
    let identity = repo.effective_identity()?;
    // D1: SSH host aliases from ~/.ssh/config, re-read when it changed.
    let aliases = githop_core::aliases::current(store.paths(), git, store.settings());
    let res = resolve(&snapshot, repo.root(), set, &aliases);
    let profile = res.profile_id.as_deref().and_then(|id| set.get(id));
    // ADR 0006: GitHub HTTPS remotes don't use the profile's SSH key.
    let https = githop_core::remote_ssh::https_remotes(&snapshot);
    let ssh_command = snapshot.get("core.sshCommand").map(str::to_string);
    let repo_key = ssh_command
        .as_deref()
        .and_then(core_pin::key_from_ssh_command);
    let key_matches = profile.map(|p| {
        repo_key
            .as_deref()
            .is_some_and(|k| same_path(k, &key_display(&p.ssh_key)))
    });
    let id_matches = match (profile, &identity) {
        (Some(p), Some(i)) => Some(identity_matches(i, p)),
        _ => None,
    };
    // Review 2026-11-01 #1: the hooksPath git really uses here (includes and
    // includeIf evaluated for this repo). Repo-local values are already a
    // resolution warning (C3).
    let hooks_override = store
        .settings()
        .hooks_installed
        .then(|| hooks::repo_hooks_override(&snapshot, store.paths(), git.home_dir().as_deref()))
        .flatten()
        .filter(|o| !matches!(o.scope, ConfigScope::Local | ConfigScope::Worktree));

    let ui = &ctx.ui;
    if ui.json {
        let warnings: Vec<serde_json::Value> =
            res.warnings.iter().map(|w| warning_json(w, set)).collect();
        ui.json_out(&json!({
            "repo": to_slash(repo.root()),
            "profile": profile.map(|p| json!({
                "id": p.id, "label": p.label, "name": p.name, "email": p.email,
                "ssh_key": key_display(&p.ssh_key),
            })),
            "source": res.source,
            "pinned": res.pinned,
            "resolution": res,
            "identity": identity,
            "identity_matches": id_matches,
            "ssh_command": ssh_command,
            "ssh_key": repo_key,
            "key_matches": key_matches,
            "warnings": warnings,
            "hooks_override": hooks_override,
            "https_remotes": https,
        }))?;
        return Ok(Exit::Ok);
    }

    ui.line(&row("Repo", &to_slash(repo.root())));
    let profile_text = match profile {
        Some(p) => format!("{} — {}", p.label, describe_source(&res)),
        None => "none — no rule matched".to_string(),
    };
    ui.line(&row("Profile", &profile_text));
    let pin_text = match &res.pinned {
        Some(p) if set.get(p).is_some() => format!("pinned to {}", super::label_for(set, p)),
        Some(p) => format!("pinned to \"{p}\" (profile no longer exists)"),
        None => "not pinned".to_string(),
    };
    ui.line(&row("Pin", &pin_text));
    match &identity {
        Some(i) if i.author == i.committer => ui.line(&row("Identity", &ident_text(&i.author))),
        Some(i) => {
            ui.line(&row("Author", &ident_text(&i.author)));
            ui.line(&row("Committer", &ident_text(&i.committer)));
        }
        None => ui.line(&row("Identity", "not set")),
    }
    ui.line(&row(
        "Key",
        repo_key
            .as_deref()
            .unwrap_or("none (ssh picks a key itself)"),
    ));

    // No rule matched this repo's GitHub owner while another profile has no owners:
    // name the owner and the one command that routes it. Pinning to the default
    // would make the mix-up permanent, so that hint is left out then.
    let owner_hint = owner_hint_lines(set, &res, profile);
    let pin_hint = |id: &str| {
        if owner_hint.is_none() {
            ui.hint(&format!("Run: githop pin {id}"));
        }
    };
    match (profile, &identity) {
        (Some(p), Some(i)) if identity_matches(i, p) => {
            ui.ok(&format!("Commits here use the {} identity.", p.label));
        }
        (Some(p), Some(i)) => {
            let wrong = if i.author.email_matches(&p.email) {
                &i.committer.email
            } else {
                &i.author.email
            };
            ui.blocked(&format!(
                "This repo belongs to {}, but commits would use {wrong}.",
                p.label
            ));
            // D8: when pinned, name what overrides the pin instead of a no-op pin.
            match res
                .pinned
                .as_ref()
                .and_then(|_| githop_core::check::commit::identity_override(&snapshot, &p.email))
            {
                Some(o) => {
                    ui.hint(&o.why());
                    ui.hint(&o.action());
                }
                None => pin_hint(&p.id),
            }
        }
        (Some(p), None) => {
            ui.warn("Git has no identity for this repo yet, so commits would fail.");
            pin_hint(&p.id);
        }
        (None, _) => {
            ui.warn("GitHop can't tell which profile owns this repo.");
            let ids = profile_ids(set);
            if ids.is_empty() {
                ui.hint("Add a profile first: githop init");
            } else {
                ui.hint(&format!("Run: githop pin <profile>  ({})", ids.join(", ")));
            }
        }
    }
    if let Some((warn, hint)) = &owner_hint {
        ui.warn(&wrap(warn, crate::ui::MAX_WIDTH - 2, "  "));
        ui.fix_hint(hint);
    }
    if let (Some(p), Some(false)) = (profile, key_matches) {
        ui.warn(&format!(
            "This repo doesn't use the {} SSH key, so a push may sign in as the wrong account.",
            p.label
        ));
        pin_hint(&p.id);
    }
    if let (Some(p), None) = (profile, &res.pinned) {
        let default_only = res.source == Some(ResolutionSource::Default);
        if id_matches == Some(true) && (key_matches != Some(false) || default_only) {
            if owner_hint.is_some() {
                // Already explained above.
            } else if res.source == Some(ResolutionSource::Default) {
                // D21: a default-only pin is what ADR 0003 #6 avoids.
                ui.hint(
                    "Only the default profile matches. Add an owner or folder rule, \
                     or pin it if that's right: githop pin <profile>",
                );
            } else {
                ui.hint(&format!("Not pinned yet. Pin it now: githop pin {}", p.id));
            }
        }
    }
    if let Some(p) = profile {
        super::remote::status_lines(ctx, &https, &p.label);
    }
    for w in &res.warnings {
        ui.warn(&describe_warning(w, set));
    }
    if let Some(o) = &hooks_override {
        match &o.value {
            Some(v) => ui.warn(&format!(
                "In this repo, core.hooksPath is {v} (set in {}), so GitHop's hooks don't run here.",
                if o.origin.is_empty() { "your git config" } else { &o.origin }
            )),
            None => ui.warn("In this repo, core.hooksPath isn't set, so GitHop's hooks don't run here."),
        }
        ui.hint("Run: githop doctor");
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use githop_core::{ProfileColor, RemoteInfo, ResolutionWarning};

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

    fn set(office_owners: &[&str]) -> ProfileSet {
        ProfileSet {
            version: 1,
            default: Some("personal".into()),
            profiles: vec![
                profile("office", "Office", office_owners),
                profile("personal", "Personal", &["harshaltupe12"]),
            ],
        }
    }

    fn default_only(owner: &str) -> Resolution {
        Resolution {
            profile_id: Some("personal".into()),
            source: Some(ResolutionSource::Default),
            remote_name: Some("origin".into()),
            remote: Some(RemoteInfo {
                host: "github.com".into(),
                owner: owner.into(),
                repo: "api".into(),
            }),
            pinned: None,
            warnings: vec![],
        }
    }

    #[test]
    fn hint_names_the_owner_and_the_ownerless_profile() {
        let s = set(&[]);
        let res = default_only("miraki-labs");
        let (warn, hint) = owner_hint_lines(&s, &res, s.get("personal")).unwrap();
        assert_eq!(
            warn,
            "No profile owns miraki-labs on GitHub, so this repo uses your default \
             profile (Personal)."
        );
        assert_eq!(
            hint,
            "If it's an Office repo, run: githop edit office --owner miraki-labs"
        );
    }

    #[test]
    fn no_hint_when_routed_pinned_or_not_github() {
        // Office already has an owner: the default really is the fallback.
        let routed = set(&["miraki-labs"]);
        assert!(owner_hint_lines(&routed, &default_only("other"), None).is_none());

        let s = set(&[]);
        let mut pinned = default_only("miraki-labs");
        pinned.source = Some(ResolutionSource::Pin);
        pinned.pinned = Some("personal".into());
        assert!(owner_hint_lines(&s, &pinned, None).is_none());

        let mut by_owner = default_only("harshaltupe12");
        by_owner.source = Some(ResolutionSource::Owner);
        assert!(owner_hint_lines(&s, &by_owner, None).is_none());

        let mut gitlab = default_only("team");
        gitlab.warnings = vec![ResolutionWarning::NonGithubHost {
            host: "gitlab.com".into(),
        }];
        assert!(owner_hint_lines(&s, &gitlab, None).is_none());

        let mut no_remote = default_only("x");
        no_remote.remote = None;
        assert!(owner_hint_lines(&s, &no_remote, None).is_none());
    }

    #[test]
    fn hint_with_no_default_profile() {
        let mut s = set(&[]);
        s.default = None;
        let mut res = default_only("miraki-labs");
        res.profile_id = None;
        res.source = None;
        let (warn, hint) = owner_hint_lines(&s, &res, None).unwrap();
        assert_eq!(
            warn,
            "No profile owns miraki-labs on GitHub, and there's no default profile."
        );
        assert_eq!(
            hint,
            "If it's an Office repo, run: githop edit office --owner miraki-labs"
        );
    }

    #[test]
    fn article_reads_naturally() {
        assert_eq!(article("Office"), "an Office");
        assert_eq!(article("Personal"), "a Personal");
    }
}
