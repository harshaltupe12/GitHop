//! `githop use <id>` — make a profile the default profile (Plan §7.6).

use anyhow::Result;

use super::{profile_display, profile_or_usage, switch_default, Ctx};
use crate::cli::UseArgs;
use crate::exit::Exit;

pub fn run(ctx: &Ctx, args: UseArgs) -> Result<Exit> {
    ctx.reject_json("use")?;
    let mut store = ctx.store()?;
    let set = store.profiles().clone();
    let profile = profile_or_usage(&set, &args.id)?.clone();

    let outcome = switch_default(ctx, &mut store, set, &profile.id)?;

    let ui = &ctx.ui;
    if outcome.first_backup {
        ui.ok("Backup saved. You can restore your original settings any time.");
    }
    ui.ok(&format!("Default profile: {}", profile_display(&profile)));
    ui.hint("Pinned repos keep their own profile.");
    Ok(Exit::Ok)
}
