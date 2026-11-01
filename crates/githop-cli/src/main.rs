//! `githop` — command-line tool and git hook entry point.
//!
//! Exit codes: `0` ok · `1` blocked / a check found a problem · `2` usage or
//! validation error · `3` internal error (hooks treat it as fail-open).

mod cli;
mod cmd;
mod exit;
mod ui;

use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::Parser;

use crate::exit::Exit;

/// Message shown instead of a panic backtrace.
const PANIC_MESSAGE: &str = "GitHop hit an internal error. Run: githop doctor";

/// Never show a raw panic: print one calm line and exit 3 (fail-open for hooks).
/// Runs before `panic = "abort"` would kill the process in release builds.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{PANIC_MESSAGE}");
        if std::env::var_os("GITHOP_DEBUG").is_some() {
            eprintln!("{info}");
        }
        std::process::exit(Exit::Internal as i32);
    }));
}

fn main() -> ExitCode {
    install_panic_hook();

    let cli = match cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let code = match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => Exit::Ok,
                _ => Exit::Usage,
            };
            let _ = err.print();
            return code.into();
        }
    };

    let ctx = cmd::Ctx::new(ui::Ui::new(&cli.global));
    match cmd::run(&ctx, cli.command) {
        Ok(code) => code.into(),
        Err(err) => exit::report(&ctx.ui, &err).into(),
    }
}
