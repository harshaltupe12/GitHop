//! `githop add` — add a profile (flags, or prompts when stdin is a terminal).

use std::path::PathBuf;

use anyhow::Result;
use githop_core::{ssh, Profile, ProfileColor, ProfileSet, Severity};

use super::{label_from_id, looks_personal, profile_display, slug, split_list, Ctx, NOREPLY_TIP};
use crate::cli::AddArgs;
use crate::exit::{missing_fields, usage, usage_hint, Exit};

pub fn run(ctx: &Ctx, args: AddArgs) -> Result<Exit> {
    ctx.reject_json("add")?;
    if !ctx.ui.interactive {
        // No terminal to ask in: report every missing flag up front.
        let mut missing = Vec::new();
        if args.id.as_deref().unwrap_or("").trim().is_empty()
            && args.label.as_deref().unwrap_or("").trim().is_empty()
        {
            missing.push("--id (or --label)");
        }
        if args.name.as_deref().unwrap_or("").trim().is_empty() {
            missing.push("--name");
        }
        if args.email.as_deref().unwrap_or("").trim().is_empty() {
            missing.push("--email");
        }
        if !missing.is_empty() {
            return Err(missing_fields(&missing));
        }
    }
    let git = ctx.git()?;
    let mut store = ctx.store()?;
    let set = store.profiles().clone();
    let ui = &ctx.ui;

    let AddArgs {
        mut id,
        mut label,
        mut name,
        mut email,
        color,
        mut owners,
        mut folders,
        key,
    } = args;

    let mut color = color.as_deref().and_then(ProfileColor::parse);
    let mut key = key;

    if ui.interactive {
        if id.is_none() {
            id = Some(ui.prompt("Profile id (e.g. office, personal)", None)?);
        }
        let id_now = id.clone().unwrap_or_default();
        if label.is_none() {
            label = Some(ui.prompt("Label", Some(&label_from_id(&id_now)))?);
        }
        if name.is_none() {
            name = Some(ui.prompt("Name for commits (git user.name)", None)?);
        }
        if email.is_none() {
            if looks_personal(&id_now, label.as_deref().unwrap_or("")) {
                ui.hint(NOREPLY_TIP);
            }
            email = Some(ui.prompt("Email for commits (git user.email)", None)?);
        }
        if color.is_none() {
            color = Some(prompt_color(ctx, &set)?);
        }
        if owners.is_empty() {
            owners = split_list(&ui.prompt(
                "GitHub owners for this profile, comma-separated (optional)",
                None,
            )?);
        }
        if folders.is_empty() {
            folders = split_list(
                &ui.prompt("Folders for this profile, comma-separated (optional)", None)?,
            );
        }
        if key.is_none() {
            let default = ssh::default_key_path(git, &id_now)
                .map(|p| super::key_display(&p))
                .unwrap_or_default();
            let answer = ui.prompt("SSH key path", Some(&default))?;
            if !answer.is_empty() {
                key = Some(PathBuf::from(answer));
            }
        }
    }

    let id = id
        .or_else(|| label.as_deref().map(slug))
        .unwrap_or_default()
        .trim()
        .to_string();
    let name = name.unwrap_or_default().trim().to_string();
    let email = email.unwrap_or_default().trim().to_string();
    let mut missing = Vec::new();
    if id.is_empty() {
        missing.push("--id (or --label)");
    }
    if name.is_empty() {
        missing.push("--name");
    }
    if email.is_empty() {
        missing.push("--email");
    }
    if !missing.is_empty() {
        return Err(missing_fields(&missing));
    }

    if set.get(&id).is_some() {
        return Err(usage_hint(
            format!("A profile called \"{id}\" already exists."),
            format!("Change it with: githop edit {id}"),
        ));
    }

    let label = label
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| label_from_id(&id));
    let color = color
        .or_else(|| set.next_free_color())
        .unwrap_or(ProfileColor::ALL[0]);
    let ssh_key = match key {
        Some(k) => k,
        None => ssh::default_key_path(git, &id).ok_or_else(|| {
            usage("GitHop couldn't find your home folder for the SSH key. Pass --key <path>.")
        })?,
    };

    let profile = Profile {
        id: id.clone(),
        label,
        name,
        email,
        color,
        ssh_key,
        owners,
        folders,
    };
    store.update(|p, _| {
        p.profiles.push(profile);
        p.normalize();
        Ok(())
    })?;

    let saved = store.profiles().get(&id).cloned();
    report_added(ctx, &set, saved.as_ref());
    Ok(Exit::Ok)
}

fn report_added(ctx: &Ctx, set: &ProfileSet, profile: Option<&Profile>) {
    let ui = &ctx.ui;
    let Some(p) = profile else { return };
    ui.ok(&format!("Profile added: {}", profile_display(p)));
    for issue in set.issues() {
        if issue.profile_id.as_deref() == Some(p.id.as_str()) && issue.severity != Severity::Error {
            ui.warn(&issue.message);
        }
    }
    if !p.ssh_key.exists() {
        ui.hint(&format!("Create its SSH key: githop keygen {}", p.id));
    }
    if set.default.is_none() {
        ui.hint(&format!("Make it the default profile: githop use {}", p.id));
    }
}

/// Ask for a colour, listing only the free ones (Phase 2 §4.3).
fn prompt_color(ctx: &Ctx, set: &ProfileSet) -> Result<ProfileColor> {
    let used: Vec<ProfileColor> = set.profiles.iter().map(|p| p.color).collect();
    let free: Vec<ProfileColor> = ProfileColor::ALL
        .into_iter()
        .filter(|c| !used.contains(c))
        .collect();
    if free.is_empty() {
        return Ok(ProfileColor::ALL[0]);
    }
    let names: Vec<&str> = free.iter().map(|c| c.as_str()).collect();
    let answer = ctx.ui.prompt(
        &format!("Colour ({})", names.join(", ")),
        Some(free[0].as_str()),
    )?;
    match ProfileColor::parse(&answer) {
        Some(c) if free.contains(&c) => Ok(c),
        _ => Err(usage_hint(
            format!("\"{answer}\" isn't a free colour."),
            format!("Free colours: {}", names.join(", ")),
        )),
    }
}
