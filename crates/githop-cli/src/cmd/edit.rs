//! `githop edit <id>` — change a profile; offer to re-pin repos when the identity changes.

use std::path::PathBuf;

use anyhow::Result;
use githop_core::text::plural;
use githop_core::{pin as core_pin, ProfileColor};

use super::{key_display, profile_display, profile_or_usage, repos_pinned_to, split_list, Ctx};
use crate::cli::EditArgs;
use crate::exit::{usage_hint, Exit};

pub fn run(ctx: &Ctx, args: EditArgs) -> Result<Exit> {
    ctx.reject_json("edit")?;
    let ui = &ctx.ui;
    let mut store = ctx.store()?;
    let mut set = store.profiles().clone();
    let settings = store.settings().clone();
    let before = profile_or_usage(&set, &args.id)?.clone();

    let any_flag = args.label.is_some()
        || args.name.is_some()
        || args.email.is_some()
        || args.color.is_some()
        || args.key.is_some()
        || !args.owners.is_empty()
        || args.no_owners
        || !args.folders.is_empty()
        || args.no_folders;

    let mut after = before.clone();
    if any_flag {
        if let Some(v) = &args.label {
            after.label = v.clone();
        }
        if let Some(v) = &args.name {
            after.name = v.clone();
        }
        if let Some(v) = &args.email {
            after.email = v.clone();
        }
        if let Some(c) = args.color.as_deref().and_then(ProfileColor::parse) {
            after.color = c;
        }
        if let Some(k) = &args.key {
            after.ssh_key = k.clone();
        }
        if args.no_owners {
            after.owners.clear();
        } else if !args.owners.is_empty() {
            after.owners = args.owners.clone();
        }
        if args.no_folders {
            after.folders.clear();
        } else if !args.folders.is_empty() {
            after.folders = args.folders.clone();
        }
    } else if ui.interactive {
        ui.hint("Press Enter to keep a value. Type - to clear a list.");
        after.label = ui.prompt("Label", Some(&before.label))?;
        after.name = ui.prompt("Name for commits (git user.name)", Some(&before.name))?;
        after.email = ui.prompt("Email for commits (git user.email)", Some(&before.email))?;
        let colour = ui.prompt("Colour", Some(before.color.as_str()))?;
        after.color = ProfileColor::parse(&colour).ok_or_else(|| {
            usage_hint(
                format!("\"{colour}\" isn't a colour GitHop knows."),
                "Colours: blue, violet, pink, indigo, plum, slate, navy, steel",
            )
        })?;
        after.owners = split_list(&ui.prompt(
            "GitHub owners, comma-separated",
            Some(&before.owners.join(", ")),
        )?);
        after.folders =
            split_list(&ui.prompt("Folders, comma-separated", Some(&before.folders.join(", ")))?);
        let key = ui.prompt("SSH key path", Some(&key_display(&before.ssh_key)))?;
        after.ssh_key = PathBuf::from(key);
    } else {
        return Err(usage_hint(
            "Nothing to change.",
            format!(
                "Pass what to change, e.g. githop edit {} --email you@example.com",
                before.id
            ),
        ));
    }

    if let Some(slot) = set.get_mut(&before.id) {
        *slot = after;
    }
    set.normalize();
    let after = profile_or_usage(&set, &before.id)?.clone();
    if after == before {
        ui.ok(&format!("No changes to {}.", after.label));
        return Ok(Exit::Ok);
    }
    let replacement = after.clone();
    store.update(|p, _| {
        if let Some(slot) = p.get_mut(&replacement.id) {
            *slot = replacement;
        }
        p.normalize();
        Ok(())
    })?;
    ui.ok(&format!("Profile updated: {}", profile_display(&after)));
    ctx.refresh_aliases(store.paths());
    if after.owners != before.owners || after.folders != before.folders {
        // D7: pins that the changed rules now disagree with.
        report_pin_disagreements(ctx, &store, &settings.pinned_repos);
    }

    let identity_changed = after.name != before.name
        || !after.email.eq_ignore_ascii_case(&before.email)
        || after.ssh_key != before.ssh_key;
    if !identity_changed {
        return Ok(Exit::Ok);
    }

    if set.default.as_deref() == Some(after.id.as_str()) {
        ui.warn("Your global git identity still has the old details.");
        ui.hint(&format!("Update it: githop use {}", after.id));
    }

    let git = ctx.git()?;
    let repos = repos_pinned_to(git, &settings.pinned_repos, &after.id);
    if repos.is_empty() {
        return Ok(Exit::Ok);
    }
    let n = repos.len();
    let repos_text = plural(n, "repo", "repos");
    let repin = args.repin
        || (ui.interactive
            && ui.confirm(
                &format!(
                    "Re-pin {repos_text} pinned to {} with the new details?",
                    after.label
                ),
                true,
            )?);
    if !repin {
        ui.warn(&format!(
            "{repos_text} pinned to {} still {} the old details.",
            after.label,
            if n == 1 { "uses" } else { "use" }
        ));
        ui.hint(&format!("Update them: githop edit {} --repin", after.id));
        return Ok(Exit::Ok);
    }
    let mut done = 0;
    for repo in &repos {
        match core_pin::pin_repo(repo, &after) {
            Ok(()) => done += 1,
            Err(e) => ui.warn(&format!(
                "Couldn't re-pin {}: {e}",
                githop_core::paths::to_slash(repo.root())
            )),
        }
    }
    ui.ok(&format!(
        "Re-pinned {} to {}.",
        plural(done, "repo", "repos"),
        after.label
    ));
    Ok(Exit::Ok)
}

/// Warn about each pinned repo whose owner or folder rule now names another
/// profile (regression D7). One config read per pinned repo.
pub fn report_pin_disagreements(ctx: &Ctx, store: &githop_core::Store, pinned: &[String]) {
    let Ok(git) = ctx.git() else { return };
    let aliases = githop_core::aliases::current(store.paths(), git, store.settings());
    for root in pinned {
        let path = std::path::Path::new(root);
        if !path.exists() {
            continue;
        }
        let Ok(repo) = githop_core::Repo::discover(git, path) else {
            continue;
        };
        let Ok(snap) = repo.config_snapshot() else {
            continue;
        };
        let res = githop_core::resolve(&snap, repo.root(), store.profiles(), &aliases);
        for w in &res.warnings {
            if matches!(w, githop_core::ResolutionWarning::PinOwnerDisagree { .. }) {
                ctx.ui.warn(&format!(
                    "{}: {}",
                    githop_core::paths::to_slash(repo.root()),
                    super::describe_warning(w, store.profiles())
                ));
            }
        }
    }
}
