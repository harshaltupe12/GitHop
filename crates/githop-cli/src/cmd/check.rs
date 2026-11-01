//! `githop check --stage <clone|commit|push> [-- <hook args>...]` — hook entry point.
//!
//! STUB: Phase 3 replaces this with the real check. Until then it never blocks
//! (hooks fail open — CLAUDE.md rule 5).

use anyhow::Result;

use super::Ctx;
use crate::cli::CheckArgs;
use crate::exit::Exit;

pub fn run(_ctx: &Ctx, args: CheckArgs) -> Result<Exit> {
    let _ = (args.stage, args.hook_args);
    eprintln!("githop check: not implemented yet");
    Ok(Exit::Ok)
}
