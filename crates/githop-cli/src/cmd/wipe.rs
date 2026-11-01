//! `githop wipe [--yes] [--delete-keys] [--keep-pins] [--keep-data] [--from-uninstaller]`
//! — confirm, then run `githop_core::wipe::wipe` and render its report
//! (Plan §7.7, Phase 6 §4).
//!
//! Exit code: `0` when every step worked, `3` when a step didn't finish
//! (running wipe again completes it).

use anyhow::Result;
use githop_core::paths::to_slash;
use githop_core::wipe::{self, WipeOptions, WipeReport, STEP_DATA};
use githop_core::{Paths, Store};

use super::Ctx;
use crate::cli::WipeArgs;
use crate::exit::{usage_hint, Exit};

/// Word the user must type to confirm.
pub const CONFIRM_WORD: &str = "WIPE";

pub fn run(ctx: &Ctx, args: WipeArgs) -> Result<Exit> {
    let ui = &ctx.ui;
    let yes = args.yes || args.from_uninstaller;
    if !yes && (!ui.interactive || ui.json) {
        return Err(usage_hint(
            "Wipe needs confirmation, and there's no terminal to ask in.",
            "Run: githop wipe --yes",
        ));
    }
    let git = ctx.git()?;
    let paths = Paths::resolve()?;

    // Read leniently: a broken store must not stop the escape hatch.
    let pinned = match Store::open_at(paths.clone()) {
        Ok(store) => store.settings().pinned_repos.len(),
        Err(e) => {
            ui.warn(&format!("GitHop data can't be read ({e}). Wiping anyway."));
            0
        }
    };

    let delete_keys = args.delete_keys && !args.from_uninstaller;
    let mut unpin = !args.keep_pins;
    if !yes {
        ui.line("Wipe will:");
        ui.line("  • remove GitHop's hooks and restore your previous core.hooksPath");
        ui.line("  • restore the global git settings GitHop changed");
        ui.line("  • remove GitHop from Windows startup");
        if args.keep_data {
            ui.line(&format!(
                "  • keep GitHop data in {}",
                to_slash(paths.root())
            ));
        } else {
            ui.line(&format!(
                "  • delete GitHop data in {}",
                to_slash(paths.root())
            ));
        }
        if delete_keys {
            ui.line("  • delete SSH keys GitHop created (~/.ssh/githop_*, and copies");
            ui.line("    import moved aside as githop_*.replaced-*)");
        } else {
            ui.line("  • keep your SSH keys");
        }
        let answer = ui.prompt(&format!("Type {CONFIRM_WORD} to continue"), None)?;
        if answer != CONFIRM_WORD {
            ui.line("Nothing was changed.");
            return Ok(Exit::Ok);
        }
        if unpin && pinned > 0 {
            unpin = ui.confirm(
                &format!(
                    "Also unpin the {} GitHop pinned?",
                    githop_core::text::plural(pinned, "repo", "repos")
                ),
                true,
            )?;
        }
    }

    let opts = WipeOptions {
        unpin_repos: unpin,
        delete_keys,
        keep_data: args.keep_data,
    };
    let report = wipe::wipe(&paths, git, &opts)?;
    let code = if report.is_complete() {
        Exit::Ok
    } else {
        Exit::Internal
    };
    if ui.json {
        ui.json_out(&serde_json::to_value(&report)?)?;
        return Ok(code);
    }
    print_report(ctx, &report, &opts);
    Ok(code)
}

fn print_report(ctx: &Ctx, report: &WipeReport, opts: &WipeOptions) {
    let ui = &ctx.ui;
    for s in &report.steps {
        if s.ok {
            ui.ok(&s.detail);
        } else {
            ui.blocked(&s.detail);
        }
    }
    if report.is_complete() {
        if opts.keep_data {
            ui.ok("GitHop wiped. Your GitHop data was kept.");
        } else {
            // D20: say exactly what went (see the unpin step for repo data).
            ui.ok("GitHop wiped. GitHop's data folder files were deleted.");
        }
        if !opts.delete_keys {
            ui.hint("Your SSH keys were kept.");
        }
    } else {
        let data_gone = report.step(STEP_DATA).is_some_and(|s| s.ok);
        if data_gone {
            ui.warn("GitHop was wiped, but some steps above need your attention.");
        } else {
            ui.blocked("Wipe didn't finish. Your git settings are safe to retry.");
        }
        ui.hint("Run: githop wipe --yes (it picks up where it stopped)");
    }
}
