//! `githop wipe [--yes] [--delete-keys]` — restore original git settings and delete
//! GitHop data (Plan §7.7). Partial: Phase 3 adds hook removal, Phase 6 the rest.
//!
//! Only the global keys GitHop changed (`settings.managed_global_keys`) are restored
//! from `gitconfig.original` — the user may have made unrelated changes since.
//! `gitconfig.original` is deleted (with the data dir) only after restore succeeded.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use githop_core::paths::to_slash;
use githop_core::{backup, gitconfig, pin as core_pin, ssh, Git, Paths, Repo, Store};

use super::{Ctx, MANAGED_GLOBAL_KEYS};
use crate::cli::WipeArgs;
use crate::exit::{usage_hint, Exit};

/// Word the user must type to confirm.
pub const CONFIRM_WORD: &str = "WIPE";

/// Read one value from a backup file: `git config --file <backup> --get <key>`.
/// `Ok(None)` when the backup doesn't set it (git exit 1).
pub fn read_backup_value(git: &Git, file: &Path, key: &str) -> Result<Option<String>> {
    let file_arg = file.to_string_lossy();
    let cwd = file.parent().unwrap_or_else(|| Path::new("."));
    let out = git.output(cwd, &["config", "--file", &file_arg, "--get", key], None)?;
    match out.status.code() {
        Some(0) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let value = text.strip_suffix('\n').unwrap_or(&text);
            let value = value.strip_suffix('\r').unwrap_or(value);
            Ok(Some(value.to_string()))
        }
        Some(1) => Ok(None),
        code => Err(anyhow!(
            "couldn't read {key} from {} (git exit {code:?}): {}",
            to_slash(file),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

pub fn run(ctx: &Ctx, args: WipeArgs) -> Result<Exit> {
    ctx.reject_json("wipe")?;
    let ui = &ctx.ui;
    if !args.yes && !ui.interactive {
        return Err(usage_hint(
            "Wipe needs confirmation, and there's no terminal to ask in.",
            "Run: githop wipe --yes",
        ));
    }
    let git = ctx.git()?;
    let paths = Paths::resolve()?;
    let original = backup::original_gitconfig(&paths);

    // A broken store must not stop the escape hatch: fall back to the keys `use` writes.
    let (managed, pinned, keys) = match Store::open_at(paths.clone()) {
        Ok(store) => (
            store.settings().managed_global_keys.clone(),
            store.settings().pinned_repos.clone(),
            store
                .profiles()
                .profiles
                .iter()
                .map(|p| p.ssh_key.clone())
                .collect::<Vec<PathBuf>>(),
        ),
        Err(e) => {
            ui.warn(&format!("GitHop data can't be read ({e}). Wiping anyway."));
            let managed = if original.is_some() {
                MANAGED_GLOBAL_KEYS.iter().map(|k| k.to_string()).collect()
            } else {
                Vec::new()
            };
            (managed, Vec::new(), Vec::new())
        }
    };

    if !args.yes {
        ui.line("Wipe will:");
        ui.line("  • restore the global git settings GitHop changed");
        ui.line(&format!(
            "  • delete GitHop data in {}",
            to_slash(paths.root())
        ));
        if args.delete_keys {
            ui.line("  • delete SSH keys GitHop created (~/.ssh/githop_*)");
        } else {
            ui.line("  • keep your SSH keys");
        }
        let answer = ui.prompt(&format!("Type {CONFIRM_WORD} to continue"), None)?;
        if answer != CONFIRM_WORD {
            ui.line("Nothing was changed.");
            return Ok(Exit::Ok);
        }
    }

    // 1. Restore the global keys GitHop changed.
    if !managed.is_empty() {
        match &original {
            Some(orig) => {
                // Safety copy of the current config before writing (CLAUDE.md rule 4).
                backup::backup_global_gitconfig(&paths, git)?;
                for key in &managed {
                    match read_backup_value(git, orig, key)? {
                        Some(value) => gitconfig::set_global(git, key, &value)?,
                        None => gitconfig::unset_global(git, key)?,
                    }
                    ui.detail(&format!("Restored {key}"));
                }
                ui.ok("Your original git settings are back.");
            }
            None => {
                ui.warn(&format!(
                    "The original backup is missing, so GitHop can't restore {}. \
                     They keep their current values.",
                    managed.join(", ")
                ));
            }
        }
    }

    // Phase 3: hooks::uninstall_hooks — unset global core.hooksPath / restore
    // settings.previous_hooks_path and delete the hooks dir before the data dir goes.

    // 2. Unpin repos GitHop pinned.
    if !pinned.is_empty() {
        let n = pinned.len();
        let unpin =
            args.yes || ui.confirm(&format!("Also unpin the {n} repo(s) GitHop pinned?"), true)?;
        if unpin {
            let mut done = 0;
            for root in &pinned {
                let path = Path::new(root);
                if !path.exists() {
                    continue;
                }
                match Repo::discover(git, path).and_then(|r| core_pin::unpin_repo(&r)) {
                    Ok(true) => done += 1,
                    Ok(false) => {}
                    Err(e) => ui.warn(&format!("Couldn't unpin {root}: {e}")),
                }
            }
            ui.ok(&format!("Unpinned {done} repo(s)."));
        } else {
            ui.hint("Pinned repos keep their pins. Remove one with: githop unpin");
        }
    }

    // 3. SSH keys: only on request, and only files GitHop named (githop_*).
    if args.delete_keys {
        let ssh_dir = ssh::ssh_dir(git);
        let mut deleted = 0;
        for key in keys {
            if !is_githop_key(&key, ssh_dir.as_deref()) {
                ui.hint(&format!(
                    "Kept {} (GitHop didn't create it).",
                    to_slash(&key)
                ));
                continue;
            }
            let mut pub_key = key.clone().into_os_string();
            pub_key.push(".pub");
            for f in [key.clone(), PathBuf::from(pub_key)] {
                if f.is_file() {
                    std::fs::remove_file(&f)
                        .with_context(|| format!("couldn't delete {}", to_slash(&f)))?;
                    deleted += 1;
                }
            }
        }
        ui.ok(&format!("Deleted {deleted} SSH key file(s)."));
    }

    // 4. Delete the data dir (includes gitconfig.original — restore already succeeded).
    if paths.root().exists() {
        std::fs::remove_dir_all(paths.root())
            .with_context(|| format!("couldn't delete {}", to_slash(paths.root())))?;
    }
    ui.ok("GitHop wiped. All GitHop data was deleted.");
    if !args.delete_keys {
        ui.hint("Your SSH keys were kept.");
    }
    Ok(Exit::Ok)
}

/// A key GitHop created: named `githop_*` inside `~/.ssh`.
fn is_githop_key(key: &Path, ssh_dir: Option<&Path>) -> bool {
    let named = key
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("githop_"));
    let in_dir = match (key.parent(), ssh_dir) {
        (Some(parent), Some(dir)) => super::same_path(&to_slash(parent), &to_slash(dir)),
        _ => false,
    };
    named && in_dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_githop_named_keys_in_ssh_dir() {
        let dir = Path::new("C:/Users/me/.ssh");
        assert!(is_githop_key(
            Path::new("C:/Users/me/.ssh/githop_office"),
            Some(dir)
        ));
        #[cfg(windows)]
        assert!(is_githop_key(
            Path::new(r"c:\users\me\.ssh\githop_office"),
            Some(dir)
        ));
        assert!(!is_githop_key(
            Path::new("C:/Users/me/.ssh/id_ed25519"),
            Some(dir)
        ));
        assert!(!is_githop_key(
            Path::new("C:/elsewhere/githop_office"),
            Some(dir)
        ));
        assert!(!is_githop_key(
            Path::new("C:/Users/me/.ssh/githop_office"),
            None
        ));
    }
}
