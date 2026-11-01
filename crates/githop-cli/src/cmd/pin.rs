//! `githop pin <id>` / `githop unpin` — pin the current repo (Plan §6).

use anyhow::Result;
use githop_core::{pin as core_pin, Repo};

use super::{cwd, forget_repo, profile_or_usage, remember_repo, Ctx};
use crate::cli::PinArgs;
use crate::exit::Exit;

pub fn run_pin(ctx: &Ctx, args: PinArgs) -> Result<Exit> {
    ctx.reject_json("pin")?;
    let git = ctx.git()?;
    let repo = Repo::discover(git, &cwd()?)?;
    let mut store = ctx.store()?;
    let profile = profile_or_usage(store.profiles(), &args.id)?.clone();
    refuse_invalid(store.profiles(), &profile)?;

    core_pin::pin_repo(&repo, &profile)?;

    let mut settings = store.settings().clone();
    if remember_repo(&mut settings.pinned_repos, repo.root()) {
        store.update_settings(|s| {
            remember_repo(&mut s.pinned_repos, repo.root());
            Ok(())
        })?;
    }
    ctx.ui.ok(&format!("Pinned to {}", profile.label));
    // D8: only claim the email when nothing wider overrides the pin.
    let snapshot = repo.config_snapshot()?;
    match githop_core::check::commit::identity_override(&snapshot, &profile.email) {
        Some(o) => {
            ctx.ui.warn(&format!(
                "Commits here still won't use {}: {}",
                profile.email,
                o.why()
            ));
            ctx.ui.fix_hint(&o.action());
        }
        None => ctx
            .ui
            .hint(&format!("Commits in this repo now use {}.", profile.email)),
    }
    Ok(Exit::Ok)
}

/// Refuse to act on a profile whose own fields don't validate (a hand-edited
/// `profiles.toml`, security regression #4).
pub fn refuse_invalid(set: &githop_core::ProfileSet, profile: &githop_core::Profile) -> Result<()> {
    match githop_core::profile::profile_errors(set, &profile.id) {
        Some(msg) => Err(crate::exit::usage_hint(
            format!("Profile {} isn't valid: {msg}", profile.id),
            "Run: githop doctor",
        )),
        None => Ok(()),
    }
}

pub fn run_unpin(ctx: &Ctx) -> Result<Exit> {
    ctx.reject_json("unpin")?;
    let git = ctx.git()?;
    let repo = Repo::discover(git, &cwd()?)?;
    let removed = core_pin::unpin_repo(&repo)?;

    let mut store = ctx.store()?;
    let mut settings = store.settings().clone();
    if forget_repo(&mut settings.pinned_repos, repo.root()) {
        store.update_settings(|s| {
            forget_repo(&mut s.pinned_repos, repo.root());
            Ok(())
        })?;
    }
    if removed {
        ctx.ui
            .ok("Unpinned. This repo now follows owner, folder and default rules.");
    } else {
        ctx.ui
            .line("This repo isn't pinned, so there was nothing to remove.");
    }
    Ok(Exit::Ok)
}
