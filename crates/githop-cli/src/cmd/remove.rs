//! `githop remove <id>` — remove a profile. Its SSH key and repo pins are left alone.

use anyhow::Result;

use super::{
    profile_display, profile_ids, profile_or_usage, repos_pinned_to, require_confirmation,
    switch_default, Ctx,
};
use crate::cli::RemoveArgs;
use crate::exit::{usage, usage_hint, Exit};

pub fn run(ctx: &Ctx, args: RemoveArgs) -> Result<Exit> {
    ctx.reject_json("remove")?;
    let ui = &ctx.ui;
    let mut store = ctx.store()?;
    let mut set = store.profiles().clone();
    let profile = profile_or_usage(&set, &args.id)?.clone();
    let is_default = set.default.as_deref() == Some(profile.id.as_str());
    let others: Vec<String> = profile_ids(&set)
        .into_iter()
        .filter(|id| *id != profile.id)
        .collect();

    // The default profile can only go when another one takes over (or it's the last one).
    let mut new_default: Option<String> = None;
    if is_default && !others.is_empty() {
        match &args.new_default {
            Some(nd) if *nd == profile.id => {
                return Err(usage(
                    "The new default profile must be a different profile.",
                ));
            }
            Some(nd) => {
                profile_or_usage(&set, nd)?;
                new_default = Some(nd.clone());
            }
            None if ui.interactive && !args.yes => {
                ui.warn(&format!("{} is the default profile.", profile.label));
                let answer = ui.prompt(
                    &format!("New default profile ({})", others.join(", ")),
                    others.first().map(String::as_str),
                )?;
                if !others.contains(&answer) {
                    return Err(usage_hint(
                        format!("There's no other profile called \"{answer}\"."),
                        format!("Profiles: {}", others.join(", ")),
                    ));
                }
                new_default = Some(answer);
            }
            None => {
                return Err(usage_hint(
                    format!(
                        "{} is the default profile, so it can't be removed yet.",
                        profile.label
                    ),
                    format!(
                        "Choose another default first: githop remove {} --yes --new-default {}",
                        profile.id,
                        others.first().map(String::as_str).unwrap_or("<id>")
                    ),
                ));
            }
        }
    } else if args.new_default.is_some() && !is_default {
        return Err(usage(format!(
            "{} isn't the default profile, so --new-default isn't needed.",
            profile.label
        )));
    }

    let question = format!(
        "Remove profile {}? Its SSH key stays in place.",
        profile_display(&profile)
    );
    if !require_confirmation(ctx, args.yes, &question, "Run again with --yes.")? {
        ui.line("Nothing was removed.");
        return Ok(Exit::Ok);
    }

    // Count repos that will become stale-pinned (before the profile disappears).
    let stale = match ctx.git() {
        Ok(git) => repos_pinned_to(git, &store.settings().pinned_repos, &profile.id).len(),
        Err(_) => 0,
    };

    set.profiles.retain(|p| p.id != profile.id);
    match &new_default {
        Some(nd) => {
            switch_default(ctx, &mut store, set.clone(), nd)?;
        }
        None => {
            if is_default {
                set.default = None;
            }
            let settings = store.settings().clone();
            store.save(&set, &settings)?;
        }
    }

    ui.ok(&format!("Profile removed: {}", profile.label));
    if let Some(nd) = &new_default {
        let label = set
            .get(nd)
            .map(|p| p.label.clone())
            .unwrap_or_else(|| nd.clone());
        ui.ok(&format!("Default profile: {label}"));
    } else if is_default {
        ui.warn("You have no default profile now. Your global git identity was left as it is.");
    }
    ui.hint(&format!(
        "The SSH key {} was kept.",
        super::key_display(&profile.ssh_key)
    ));
    if stale > 0 {
        ui.warn(&format!(
            "{stale} repo(s) are still pinned to {}. Commits there will be checked against \
             other rules until you pin them again.",
            profile.label
        ));
        ui.hint("Pin each one: githop pin <profile>");
    }
    Ok(Exit::Ok)
}
