//! `githop init` — detect the current global identity, back up the global git
//! config, create the first profile from it, then install the hooks (Phase 3
//! §4.4). Safe to run again (idempotent): a rerun changes no profile and
//! re-installs the hooks with the running `githop.exe`.

use std::path::PathBuf;

use anyhow::Result;
use githop_core::profile::{is_public_mail, suggest_id};
use githop_core::{backup, gitconfig, pin as core_pin, ssh, Profile, ProfileColor, Store};

use super::hooks::{install, print_install, Ask};
use super::{key_display, label_from_id, profile_display, Ctx, NOREPLY_TIP};
use crate::cli::InitArgs;
use crate::exit::{missing_fields, Exit};

pub fn run(ctx: &Ctx, args: InitArgs) -> Result<Exit> {
    ctx.reject_json("init")?;
    let ui = &ctx.ui;
    let git = ctx.git()?;
    let mut store = ctx.store()?;
    let paths = store.paths().clone();

    // Already set up → report, make sure the original backup exists, change nothing else.
    if !store.profiles().profiles.is_empty() {
        if backup::original_gitconfig(&paths).is_none() {
            paths.ensure_dirs()?;
            backup::backup_global_gitconfig(&paths, git)?;
            ui.ok("Backup saved. You can restore your original settings any time.");
        }
        let set = store.profiles();
        match set.default_profile() {
            Some(p) => ui.ok(&format!(
                "GitHop is already set up. Default profile: {}",
                profile_display(p)
            )),
            None => ui.ok(&format!(
                "GitHop is already set up with {}.",
                githop_core::text::plural(set.profiles.len(), "profile", "profiles")
            )),
        }
        install_hooks_step(ctx, &store, &args)?;
        ui.hint("Add another profile: githop add");
        return Ok(Exit::Ok);
    }

    let current_name = gitconfig::get_global(git, "user.name")?;
    let current_email = gitconfig::get_global(git, "user.email")?;
    let current_key = gitconfig::get_global(git, "core.sshCommand")?
        .as_deref()
        .and_then(core_pin::key_from_ssh_command);
    match (&current_name, &current_email) {
        (Some(n), Some(e)) => ui.line(&format!("Your current git identity: {n} <{e}>")),
        _ => ui.line("No global git identity is set yet."),
    }

    let mut id = args.id.clone();
    let mut name = args.name.clone().or(current_name.clone());
    let mut email = args.email.clone().or(current_email.clone());
    let mut label = args.label.clone();
    if ui.interactive {
        if id.is_none() {
            let suggested = current_email.as_deref().and_then(suggest_id);
            id = Some(ui.prompt(
                "Profile id for this identity (e.g. work, personal, client)",
                suggested.as_deref(),
            )?);
        }
        let id_now = id.clone().unwrap_or_default();
        if label.is_none() {
            label = Some(ui.prompt("Label", Some(&label_from_id(&id_now)))?);
        }
        if args.name.is_none() {
            name = Some(ui.prompt("Name for commits", current_name.as_deref())?);
        }
        if args.email.is_none() {
            if current_email.as_deref().is_none_or(is_public_mail) {
                ui.hint(NOREPLY_TIP);
            }
            email = Some(ui.prompt("Email for commits", current_email.as_deref())?);
        }
    }

    let id = id.unwrap_or_default().trim().to_string();
    let name = name.unwrap_or_default().trim().to_string();
    let email = email.unwrap_or_default().trim().to_string();
    let mut missing = Vec::new();
    if id.is_empty() {
        missing.push("--id");
    }
    if name.is_empty() {
        missing.push("--name (no global user.name to copy)");
    }
    if email.is_empty() {
        missing.push("--email (no global user.email to copy)");
    }
    if !missing.is_empty() {
        return Err(missing_fields(&missing));
    }

    let ssh_key: PathBuf = match (&args.key, &current_key) {
        (Some(k), _) => k.clone(),
        (None, Some(k)) => PathBuf::from(k),
        (None, None) => ssh::default_key_path(git, &id).unwrap_or_default(),
    };
    let profile = Profile {
        id: id.clone(),
        label: label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| label_from_id(&id)),
        name,
        email,
        color: args
            .color
            .as_deref()
            .and_then(ProfileColor::parse)
            .unwrap_or(ProfileColor::ALL[0]),
        ssh_key,
        owners: Vec::new(),
        folders: Vec::new(),
        sign_commits: false,
    };

    // Regression D17: check everything before writing anything (no backup
    // or data folder for input that's refused anyway).
    let mut check = store.profiles().clone();
    check.profiles.push(profile.clone());
    check.default = Some(id.clone());
    check.normalize();
    check
        .validate()
        .map_err(githop_core::CoreError::Validation)?;

    if ui.interactive
        && !args.yes
        && !ui.confirm(
            &format!(
                "Create profile {} and make it the default profile?",
                profile_display(&profile)
            ),
            true,
        )?
    {
        ui.line("Nothing was changed.");
        return Ok(Exit::Ok);
    }

    // Back up before anything else touches git config (CLAUDE.md rule 4).
    paths.ensure_dirs()?;
    let first_backup = backup::original_gitconfig(&paths).is_none();
    backup::backup_global_gitconfig(&paths, git)?;
    if first_backup {
        ui.ok("Backup saved. You can restore your original settings any time.");
    }

    // The profile mirrors the current global identity, so it becomes the default
    // without writing global config. (`githop use` sets the SSH key globally.)
    let mut set = store.profiles().clone();
    set.profiles.push(profile);
    set.default = Some(id.clone());
    set.normalize();
    let new_profile = set.get(&id).cloned();
    store.update(|p, _| {
        if let Some(np) = new_profile {
            p.profiles.push(np);
        }
        p.default = Some(id.clone());
        p.normalize();
        Ok(())
    })?;

    let saved = set.get(&id).cloned();
    if let Some(p) = &saved {
        ui.ok(&format!("Profile created: {}", profile_display(p)));
        ui.ok(&format!("Default profile: {}", p.label));
        if !p.ssh_key.exists() {
            ui.hint(&format!(
                "No SSH key at {} yet. Create one: githop keygen {}",
                key_display(&p.ssh_key),
                p.id
            ));
        }
    }
    let identity_differs = current_email.as_deref().is_some_and(|e| {
        saved
            .as_ref()
            .is_some_and(|p| !p.email.eq_ignore_ascii_case(e))
    }) || current_email.is_none();
    if identity_differs {
        ui.hint(&format!("Set it as your global identity: githop use {id}"));
    }
    install_hooks_step(ctx, &store, &args)?;
    ui.hint("Next: add your other profile with githop add, then githop scan <folder>.");
    Ok(Exit::Ok)
}

/// Last step: install (or refresh) the hooks. Explain and ask the first time
/// when there's a terminal; `--yes` skips the question, `--no-hooks` the step.
fn install_hooks_step(ctx: &Ctx, store: &Store, args: &InitArgs) -> Result<()> {
    if args.no_hooks {
        if let Ok(git) = ctx.git() {
            githop_core::aliases::refresh_best_effort(store.paths(), git);
        }
        if !store.settings().hooks_installed {
            ctx.ui.hint(
                "Hooks weren't installed, so commits and pushes aren't checked. \
                 Install them: githop hooks install",
            );
        }
        return Ok(());
    }
    let ask = if store.settings().hooks_installed || args.yes {
        Ask::Quiet
    } else {
        Ask::Confirm
    };
    if let Some(done) = install(ctx, store.paths(), ask)? {
        print_install(ctx, &done);
    }
    Ok(())
}
