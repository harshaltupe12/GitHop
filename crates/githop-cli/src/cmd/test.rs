//! `githop test <id>` — `ssh -T git@github.com` with the profile key.
//!
//! **Network: user-initiated only** (Plan §10). This is the one command that
//! connects anywhere, and only because the user asked for it.

use anyhow::Result;
use githop_core::ssh::{self, ConnectionTest};
use serde_json::json;

use super::{key_display, profile_or_usage, Ctx};
use crate::cli::TestArgs;
use crate::exit::{usage_hint, Exit};

pub fn run(ctx: &Ctx, args: TestArgs) -> Result<Exit> {
    let ui = &ctx.ui;
    let store = ctx.store()?;
    let profile = profile_or_usage(store.profiles(), &args.id)?.clone();
    let key = profile.ssh_key.clone();
    if !key.is_file() {
        return Err(usage_hint(
            format!("There's no SSH key at {}.", key_display(&key)),
            format!("Create one: githop keygen {}", profile.id),
        ));
    }

    // GitHub's published host keys first, so ssh never stops at yes/no (ADR 0006).
    if let Ok(git) = ctx.git() {
        ssh::ensure_github_known_hosts_best_effort(git, store.paths());
    }
    ui.line(&format!(
        "Connecting to github.com with the {} key…",
        profile.label
    ));
    let result = ssh::test_connection(&key)?;

    let code = match &result {
        ConnectionTest::Connected { .. } => Exit::Ok,
        _ => Exit::Blocked,
    };
    if ui.json {
        ui.json_out(&json!({
            "profile": profile.id,
            "key": key_display(&key),
            "result": result,
        }))?;
        return Ok(code);
    }
    match &result {
        ConnectionTest::Connected { user } => {
            ui.ok(&format!("Connected as {user}"));
            if !profile.owners.is_empty()
                && !profile.owners.iter().any(|o| o.eq_ignore_ascii_case(user))
            {
                ui.hint(&format!(
                    "{user} isn't one of {}'s owners. Add it if this account owns repos: \
                     githop edit {} --owner {user}",
                    profile.label, profile.id
                ));
            }
        }
        ConnectionTest::Denied => {
            ui.blocked(&format!("GitHub didn't accept the {} key.", profile.label));
            ui.hint("Add the public key to your GitHub account (Settings → SSH and GPG keys), then try again.");
        }
        ConnectionTest::Failed { reason } => {
            ui.blocked(&format!("Couldn't reach GitHub: {reason}"));
            ui.hint("Check your network connection, then try again.");
        }
    }
    Ok(code)
}
