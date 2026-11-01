//! `githop keygen <id> [--no-passphrase]` — create an ed25519 key for a profile.
//!
//! `ssh-keygen` runs with the console attached, so it asks for the passphrase
//! itself — GitHop never sees it (Plan §9.2).

use anyhow::Result;
use githop_core::ssh;

use super::{key_display, profile_or_usage, Ctx};
use crate::cli::KeygenArgs;
use crate::exit::{usage, usage_hint, Exit};

pub fn run(ctx: &Ctx, args: KeygenArgs) -> Result<Exit> {
    ctx.reject_json("keygen")?;
    let ui = &ctx.ui;
    let git = ctx.git()?;
    let mut store = ctx.store()?;
    let mut set = store.profiles().clone();
    let profile = profile_or_usage(&set, &args.id)?.clone();

    let key = if profile.ssh_key.as_os_str().is_empty() {
        ssh::default_key_path(git, &profile.id)
            .ok_or_else(|| usage("GitHop couldn't find your home folder for the SSH key."))?
    } else {
        profile.ssh_key.clone()
    };
    if key.exists() {
        return Err(usage_hint(
            format!(
                "A key already exists at {}. GitHop won't overwrite it.",
                key_display(&key)
            ),
            format!(
                "Use it as it is, or pick another path: githop edit {} --key <path>",
                profile.id
            ),
        ));
    }

    if !args.no_passphrase {
        ui.hint("ssh-keygen will ask for a passphrase. GitHop never sees it.");
    }
    ssh::keygen(&key, &profile.email, args.no_passphrase)?;

    if profile.ssh_key != key {
        if let Some(p) = set.get_mut(&profile.id) {
            p.ssh_key = key.clone();
        }
        let settings = store.settings().clone();
        store.save(&set, &settings)?;
    }

    ui.ok(&format!("Key created: {}", key_display(&key)));
    if args.no_passphrase {
        ui.warn("This key has no passphrase. Anyone who can read the file can use it.");
    }
    match ssh::read_public_key(&key) {
        Ok(public) => {
            ui.line("");
            // The public key is the point of this command, so it prints even with -q.
            println!("{public}");
            ui.line("");
            ui.hint("Add it on GitHub: Settings → SSH and GPG keys → New SSH key.");
            ui.hint(&format!("Then check it: githop test {}", profile.id));
        }
        Err(e) => ui.warn(&format!("Couldn't read the public key: {e}")),
    }
    Ok(Exit::Ok)
}
