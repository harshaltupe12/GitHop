//! `githop hooks install|uninstall|status|snippet <stage>` (Phase 3 §4.1, §4.3).
//!
//! Install and uninstall back up the global git config first (CLAUDE.md
//! rule 4) and save `settings.toml` afterwards; the core does the rest.

use std::path::PathBuf;

use anyhow::Result;
use githop_core::doctor::{self, CheckStatus};
use githop_core::hooks::{self, HookHealth, HooksPathChange, InstallReport};
use githop_core::paths::to_slash;
use githop_core::{aliases, backup, check, Paths, Store};

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
fn explain(ctx: &Ctx, paths: &Paths) {
    let ui = &ctx.ui;
    let dir = hooks::hooks_dir_value(paths);
    ui.line("GitHop checks each commit and push with git hooks:");
    ui.line(&format!("  • global core.hooksPath will point to {dir}"));
    ui.line("  • your repos' own hooks and any hooks you had before keep running");
    ui.line("  • if GitHop itself has a problem, git keeps working");
    ui.line("  • undo any time: githop hooks uninstall");
}

/// What [`install`] did, and how the hooks were before (for the wording).
pub struct Installed {
    pub report: InstallReport,
    pub before: HookHealth,
}

/// Install or re-install the hooks with the running exe; saves settings.
/// Only `settings.toml` is read and written, so this works while
/// `profiles.toml` is broken (regression D5). Also refreshes the SSH host
/// alias cache (regression D1). Returns `None` when the user said no.
pub fn install(ctx: &Ctx, paths: &Paths, ask: Ask) -> Result<Option<Installed>> {
    let ui = &ctx.ui;
    let git = ctx.git()?;
    if ask != Ask::Quiet {
        explain(ctx, paths);
    }
    if ask == Ask::Confirm && ui.interactive && !ui.confirm("Install the hooks?", true)? {
        ui.line("The hooks weren't installed. Install them later: githop hooks install");
        return Ok(None);
    }
    let paths = paths.clone();
    paths.ensure_dirs()?;
    let before = hooks::verify_hooks(&paths, git, &check::load_settings_lenient(&paths));
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
    let (report, _) =
        Store::update_settings_at(&paths, |s| hooks::install_hooks(&paths, git, s, &exe))?;
    aliases::refresh_best_effort(&paths, git);
    Ok(Some(Installed { report, before }))
}

/// The headline for an install (regression D11): installed, repaired,
/// updated or already up to date, from how the hooks were before.
pub fn install_headline(before: &HookHealth) -> String {
    let mut fixed: Vec<String> = before.missing.clone();
    fixed.extend(before.tampered.iter().cloned());
    if !before.installed || !before.points_to_us || !before.overridden_by.is_empty() {
        "Hooks installed. GitHop now checks every commit and push.".to_string()
    } else if !fixed.is_empty() {
        fixed.sort();
        fixed.dedup();
        format!(
            "Hooks repaired: {}. GitHop checks every commit and push.",
            fixed.join(", ")
        )
    } else if !before.stale.is_empty() || before.exe_missing {
        "Hooks updated. GitHop checks every commit and push.".to_string()
    } else {
        "Hooks are up to date. GitHop checks every commit and push.".to_string()
    }
}

/// Print what [`install`] did.
pub fn print_install(ctx: &Ctx, installed: &Installed) {
    let ui = &ctx.ui;
    let report = &installed.report;
    ui.ok(&install_headline(&installed.before));
    if let Some(prev) = &report.previous_hooks_path {
        ui.hint(&format!("Your previous hooks in {prev} still run."));
    }
    for i in &report.included_hooks_paths {
        ui.hint(&format!(
            "{} sets core.hooksPath = {}. Those hooks still run where it applies.",
            i.describe(),
            i.value
        ));
    }
    ui.detail(&format!(
        "core.hooksPath = {} ({} files)",
        report.hooks_dir,
        report.written.len()
    ));
}

fn run_install(ctx: &Ctx, args: HooksInstallArgs) -> Result<Exit> {
    ctx.reject_json("hooks install")?;
    // Settings only: works even when profiles.toml can't be read (D5).
    let paths = Paths::resolve()?;
    if args.all_stubs || args.common_stubs {
        let all = args.all_stubs;
        Store::update_settings_at(&paths, |s| {
            s.all_hook_stubs = all;
            Ok(())
        })?;
    }
    let installed = Store::load_settings_at(&paths)?.hooks_installed;
    let ask = if args.yes || installed {
        Ask::Quiet
    } else {
        Ask::Explain
    };
    if let Some(done) = install(ctx, &paths, ask)? {
        print_install(ctx, &done);
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
    let report = store.update_settings(|s| hooks::uninstall_hooks(&paths, git, s))?;
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
                ui.fix_hint(&format!("  Fix: {fix}"));
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
    // Only the snippet on stdout, so it can be piped or copied as is.
    println!("{line}");
    if !ctx.ui.quiet {
        let hook = match args.stage {
            Stage::Clone => "post-checkout",
            Stage::Commit => "pre-commit",
            Stage::Push => "pre-push",
        };
        let text = if args.stage == Stage::Push {
            // D6: the push snippet reads stdin once and hands it back.
            format!(
                "Add it near the top of the repo's {hook} hook (e.g. .husky/{hook}). \
                 It hands the ref lines back, so the rest of the hook (git lfs pre-push…) \
                 still gets them."
            )
        } else {
            format!("Add it near the top of the repo's {hook} hook (e.g. .husky/{hook}).")
        };
        eprintln!(
            "{}",
            crate::ui::paint_dim(
                &githop_core::text::wrap(&text, crate::ui::MAX_WIDTH, ""),
                ctx.ui.color_err
            )
        );
    }
    Ok(Exit::Ok)
}
