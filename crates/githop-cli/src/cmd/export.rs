//! `githop export <file> [--include-keys]` — save every profile to a
//! password-protected file for another PC (Phase 6 §3, Plan §10 "Multi-machine").
//!
//! The password is asked twice with hidden input (or read from
//! `GITHOP_EXPORT_PASSWORD` for scripts and tests). It is kept in memory only,
//! wiped after use, and never written anywhere.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use githop_core::export::{self, ExportError, MIN_PASSWORD_CHARS};
use githop_core::Severity;
use zeroize::Zeroizing;

use super::{key_display, require_confirmation, Ctx};
use crate::exit::{sentence, usage, usage_hint, Exit, UsageError};

/// Env var with the export/import password (for scripts; skips the prompt).
pub const PASSWORD_ENV: &str = "GITHOP_EXPORT_PASSWORD";

#[derive(Debug, Clone, Args)]
pub struct ExportArgs {
    /// File to write, e.g. profiles.githop
    pub file: PathBuf,
    /// Also include your private SSH keys (passphrases stay on)
    #[arg(long)]
    pub include_keys: bool,
    /// Replace the file if it already exists, without asking
    #[arg(short, long)]
    pub yes: bool,
}

pub fn run(ctx: &Ctx, args: ExportArgs) -> Result<Exit> {
    ctx.reject_json("export")?;
    let ui = &ctx.ui;
    let git = ctx.git()?;
    let store = ctx.store()?;
    if store.profiles().profiles.is_empty() {
        return Err(usage_hint(
            "You have no profiles to export yet.",
            "Add one first: githop add",
        ));
    }

    let file = &args.file;
    if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            return Err(usage(format!(
                "The folder {} doesn't exist. Nothing was exported.",
                key_display(parent)
            )));
        }
    }
    if file.is_dir() {
        return Err(usage(format!(
            "{} is a folder. Give a file name, like profiles.githop.",
            key_display(file)
        )));
    }
    if file.exists() {
        let question = format!("{} already exists. Replace it?", key_display(file));
        if !require_confirmation(
            ctx,
            args.yes,
            &question,
            "Run again with --yes to replace it.",
        )? {
            ui.line("Nothing was exported.");
            return Ok(Exit::Ok);
        }
    }

    if args.include_keys {
        ui.warn("The file will contain your private SSH keys. Keep it somewhere safe.");
        ui.hint("Keys with a passphrase stay protected by it inside the file.");
    }
    let password = read_password(ctx, true)?;
    let summary = export::export(store.paths(), git, file, &password, args.include_keys)
        .map_err(cli_error)?;
    drop(password);

    ui.ok(&format!(
        "Exported {} to {}",
        count(summary.profiles.len(), "profile"),
        key_display(file)
    ));
    if args.include_keys {
        if summary.keys.is_empty() {
            ui.warn("No SSH keys were found to include.");
        } else {
            ui.line(&format!("SSH keys included: {}", summary.keys.join(", ")));
        }
        for missing in &summary.missing_keys {
            ui.warn(&format!(
                "Key file not found, so it wasn't included: {}",
                key_display(missing)
            ));
        }
    }
    ui.hint("Keep the password safe. Without it nobody can open the file, not even GitHop.");
    ui.hint(&format!(
        "On the other PC, run: githop import {}",
        file.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| key_display(file))
    ));
    Ok(Exit::Ok)
}

/// `1 profile` / `2 profiles`.
pub fn count(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The password from [`PASSWORD_ENV`], else a hidden prompt (twice when `confirm`).
/// Fails with a usage error when there's no terminal to ask in.
pub fn read_password(ctx: &Ctx, confirm: bool) -> Result<Zeroizing<String>> {
    if let Some(value) = std::env::var_os(PASSWORD_ENV) {
        let value = Zeroizing::new(
            value
                .into_string()
                .map_err(|_| usage(format!("{PASSWORD_ENV} isn't valid text.")))?,
        );
        if !value.is_empty() {
            if confirm {
                export::check_password(&value).map_err(cli_error)?;
            }
            return Ok(value);
        }
    }
    if !ctx.ui.interactive {
        return Err(usage_hint(
            "There's no terminal to ask for the password in.",
            format!("Run this in a terminal, or set {PASSWORD_ENV}."),
        ));
    }
    let label = if confirm {
        format!("Password (at least {MIN_PASSWORD_CHARS} characters): ")
    } else {
        "Password: ".to_string()
    };
    let first = Zeroizing::new(rpassword::prompt_password(label)?);
    if confirm {
        export::check_password(&first).map_err(cli_error)?;
        let second = Zeroizing::new(rpassword::prompt_password("Type it again: ")?);
        if *first != *second {
            return Err(usage("The passwords don't match. Nothing was exported."));
        }
    }
    Ok(first)
}

/// Turn an export/import error into a CLI error: problems with the input or
/// file are usage errors (exit 2); store/file-system problems keep their
/// [`githop_core::CoreError`] so the normal reporting applies.
pub fn cli_error(e: ExportError) -> anyhow::Error {
    match e {
        ExportError::Core(core) => anyhow::Error::new(core),
        ExportError::NoHome => anyhow::Error::new(ExportError::NoHome),
        ExportError::WrongPassword => usage_hint(
            format!("{}.", sentence(&ExportError::WrongPassword.to_string())),
            "Check the password and try again. A lost password can't be recovered.",
        ),
        ExportError::WeakPassword { min } => usage_hint(
            format!("The password is too short. Use at least {min} characters."),
            "A few random words make a strong, memorable password.",
        ),
        ExportError::Unresolved(ids) => usage_hint(
            format!(
                "Some profiles in the file clash with yours: {}. Nothing was imported.",
                ids.join(", ")
            ),
            "Choose what to do: --on-conflict skip|replace|rename, or run this in a terminal to be asked.",
        ),
        ExportError::InvalidProfiles(issues) => anyhow::Error::new(UsageError {
            message: "The profiles in this file aren't valid. Nothing was imported.".to_string(),
            details: issues
                .iter()
                .filter(|i| i.severity == Severity::Error)
                .map(|i| {
                    let who = i
                        .profile_id
                        .as_deref()
                        .map(|p| format!("{p}: "))
                        .unwrap_or_default();
                    format!("{who}{} — {}", i.field, i.message)
                })
                .collect(),
            hint: None,
        }),
        other => usage(format!("{}.", sentence(&other.to_string()))),
    }
}
