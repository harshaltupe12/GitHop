//! `githop hooks install|uninstall|status|snippet <stage>` (Phase 3 §4.1, §4.3).
//!
//! Install and uninstall back up the global git config first (CLAUDE.md
//! rule 4) and save `settings.toml` afterwards; the core does the rest.

use std::path::PathBuf;

use anyhow::Result;
use githop_core::doctor::{self, CheckStatus};
use githop_core::hooks::{self, HooksPathChange, InstallReport};
use githop_core::paths::to_slash;
use githop_core::{backup, Store};

use super::Ctx;
use crate::cli::{HooksCommand, HooksInstallArgs, HooksSnippetArgs, Stage};
use crate::exit::Exit;

pub fn run(ctx: &Ctx, command: HooksCommand) -> Result<Exit> {
    match command {
        HooksCommand::Install(a) => run_install(ctx, a),
        HooksCommand::Uninstall => run_uninstall(ctx),
        HooksCommand::Status => run_status(ctx),
        HooksCommand::Snippet(a) => run_snippet(ctx, a),
    }
}

/// The running `githop.exe` (absolute), as hooks should call it.
pub fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    // `current_exe` can carry a `\\?\` prefix; git's sh can't run that.
    let s = exe.to_string_lossy();
    Ok(match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => exe,
    })
}

/// How [`install`] should ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Explain and ask (only when there's a terminal).
    Confirm,
    /// Explain, don't ask.
    Explain,
    /// Just do it (re-install / `--yes`).
    Quiet,
}

/// Lines explaining what installing hooks does (Design §9.1: say it's safe).
fn explain(ctx: &Ctx, store: &Store) {
    let ui = &ctx.ui;
    let dir = hooks::hooks_dir_value(store.paths());
    ui.line("GitHop checks each commit and push with git hooks:");
    ui.line(&format!("  • global core.hooksPath will point to {dir}"));
    ui.line("  • your repos' own hooks and any hooks you had before keep running");
    ui.line("  • if GitHop itself has a problem, git keeps working");
    ui.line("  • undo any time: githop hooks uninstall");
}

/// Install or re-install the hooks with the running exe; saves settings.
/// Returns `None` when the user said no.
pub fn install(ctx: &Ctx, store: &mut Store, ask: Ask) -> Result<Option<InstallReport>> {
    let ui = &ctx.ui;
    let git = ctx.git()?;
    if ask != Ask::Quiet {
        explain(ctx, store);
    }
    if ask == Ask::Confirm && ui.interactive && !ui.confirm("Install the hooks?", true)? {
        ui.line("The hooks weren't installed. Install them later: githop hooks install");
        return Ok(None);
    }
    let paths = store.paths().clone();
    paths.ensure_dirs()?;
    let first_backup = backup::original_gitconfig(&paths).is_none();
    let saved = backup::backup_global_gitconfig(&paths, git)?;
    if first_backup {
        ui.ok("Backup saved. You can restore your original settings any time.");
    }
    ui.detail(&format!(
        "Global git config backed up to {}",
        to_slash(&saved)
    ));
    let exe = current_exe()?;
    let mut settings = store.settings().clone();
    let report = hooks::install_hooks(&paths, git, &mut settings, &exe)?;
    store.save_settings(&settings)?;
    Ok(Some(report))
}

/// Print what [`install`] did.
pub fn print_install(ctx: &Ctx, report: &InstallReport) {
    let ui = &ctx.ui;
    if report.reinstall {
        ui.ok("Hooks are up to date. GitHop checks every commit and push.");
    } else {
        ui.ok("Hooks installed. GitHop now checks every commit and push.");
    }
    if let Some(prev) = &report.previous_hooks_path {
        ui.hint(&format!("Your previous hooks in {prev} still run."));
    }
    ui.detail(&format!(
        "core.hooksPath = {} ({} files)",
        report.hooks_dir,
        report.written.len()
    ));
}

fn run_install(ctx: &Ctx, args: HooksInstallArgs) -> Result<Exit> {
    ctx.reject_json("hooks install")?;
    let mut store = ctx.store()?;
    let ask = if args.yes || store.settings().hooks_installed {
        Ask::Quiet
    } else {
        Ask::Explain
    };
    if let Some(report) = install(ctx, &mut store, ask)? {
        print_install(ctx, &report);
    }
    Ok(Exit::Ok)
}

fn run_uninstall(ctx: &Ctx) -> Result<Exit> {
    ctx.reject_json("hooks uninstall")?;
    let ui = &ctx.ui;
    let git = ctx.git()?;
    let mut store = ctx.store()?;
    let paths = store.paths().clone();
    paths.ensure_dirs()?;
    backup::backup_global_gitconfig(&paths, git)?;
    let mut settings = store.settings().clone();
    let report = hooks::uninstall_hooks(&paths, git, &mut settings)?;
    store.save_settings(&settings)?;
    ui.ok("Hooks removed. GitHop no longer checks commits and pushes.");
    match report.hooks_path {
        HooksPathChange::Restored { value } => {
            ui.hint(&format!("core.hooksPath is back to {value}."));
        }
        HooksPathChange::Unset => ui.hint("core.hooksPath is unset, as it was before."),
        HooksPathChange::LeftAlone { value } => ui.hint(&format!(
            "core.hooksPath ({value}) was changed after GitHop set it, so it was left as is."
        )),
    }
    ui.hint("Install them again: githop hooks install");
    Ok(Exit::Ok)
}

fn run_status(ctx: &Ctx) -> Result<Exit> {
    let ui = &ctx.ui;
    let git = ctx.git()?;
    let store = ctx.store()?;
    let health = hooks::verify_hooks(store.paths(), git, store.settings());
    let has_profiles = !store.profiles().profiles.is_empty();
    let exe = current_exe().ok();
    let checks = doctor::hook_checks(&health, has_profiles, exe.as_deref());
    let problems = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Problem)
        .count();
    let code = if problems > 0 {
        Exit::Blocked
    } else {
        Exit::Ok
    };
    if ui.json {
        ui.json_out(&serde_json::json!({
            "health": health,
            "ok": health.is_ok(),
            "checks": checks,
        }))?;
        return Ok(code);
    }
    for c in &checks {
        match c.status {
            CheckStatus::Ok => ui.ok(&c.detail),
            CheckStatus::Warning => ui.warn(&c.detail),
            CheckStatus::Problem => ui.blocked(&c.detail),
        }
        if c.status != CheckStatus::Ok {
            if let Some(fix) = &c.fix {
                ui.hint(&format!("  Fix: {fix}"));
            }
        }
    }
    Ok(code)
}

fn run_snippet(ctx: &Ctx, args: HooksSnippetArgs) -> Result<Exit> {
    ctx.reject_json("hooks snippet")?;
    let store = ctx.store()?;
    let exe = if store.settings().githop_exe.is_empty() {
        to_slash(&current_exe()?)
    } else {
        store.settings().githop_exe.clone()
    };
    let stage = match args.stage {
        Stage::Clone => "clone",
        Stage::Commit => "commit",
        Stage::Push => "push",
    };
    let line = hooks::snippet(stage, &exe)?;
    // Only the line on stdout, so it can be piped or copied as is.
    println!("{line}");
    if !ctx.ui.quiet {
        let hook = match args.stage {
            Stage::Clone => "post-checkout",
            Stage::Commit => "pre-commit",
            Stage::Push => "pre-push",
        };
        eprintln!(
            "{}",
            crate::ui::paint_dim(
                &format!("Add it near the top of the repo's {hook} hook (e.g. .husky/{hook})."),
                ctx.ui.color_err
            )
        );
    }
    Ok(Exit::Ok)
}
