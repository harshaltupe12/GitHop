//! `githop status [--json]` — which identity will this repo use, and why.

use anyhow::Result;
use githop_core::paths::to_slash;
use githop_core::{pin as core_pin, resolve, EffectiveIdentity, Identity, Profile, Repo};
use serde_json::json;

use super::{
    cwd, describe_source, describe_warning, key_display, profile_ids, same_path, warning_json, Ctx,
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

pub fn run(ctx: &Ctx) -> Result<Exit> {
    let git = ctx.git()?;
    let repo = Repo::discover(git, &cwd()?)?;
    let store = ctx.store()?;
    let set = store.profiles();
    let snapshot = repo.config_snapshot()?;
    let identity = repo.effective_identity()?;
    let res = resolve(
        &snapshot,
        repo.root(),
        set,
        &store.settings().github_host_aliases,
    );
    let profile = res.profile_id.as_deref().and_then(|id| set.get(id));
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
            ui.hint(&format!("Run: githop pin {}", p.id));
        }
        (Some(p), None) => {
            ui.warn("Git has no identity for this repo yet, so commits would fail.");
            ui.hint(&format!("Run: githop pin {}", p.id));
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
    if let (Some(p), Some(false)) = (profile, key_matches) {
        ui.warn(&format!(
            "This repo doesn't use the {} SSH key, so a push may sign in as the wrong account.",
            p.label
        ));
        ui.hint(&format!("Run: githop pin {}", p.id));
    } else if let (Some(p), None) = (profile, &res.pinned) {
        if id_matches == Some(true) {
            ui.hint(&format!("Not pinned yet. Pin it now: githop pin {}", p.id));
        }
    }
    for w in &res.warnings {
        ui.warn(&describe_warning(w, set));
    }
    Ok(Exit::Ok)
}
