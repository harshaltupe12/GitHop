//! `githop` — command-line tool and git hook entry point.

use clap::Parser;

#[derive(Parser)]
#[command(name = "githop", version, about = "Hop between git identities safely")]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
