//! `githop fix` — re-stamp unpushed commits that used the wrong identity.
//!
//! STUB: Phase 4 replaces this.

use anyhow::Result;

use super::Ctx;
use crate::exit::Exit;

pub fn run(_ctx: &Ctx) -> Result<Exit> {
    eprintln!("githop fix: not implemented yet");
    Ok(Exit::Ok)
}
