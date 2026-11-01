//! `githop import <file> [--on-conflict skip|replace|rename] [--yes]` — add the
//! profiles (and SSH keys, if included) from an export file (Phase 6 §3).
//!
//! Shows what's in the file and every clash first. Existing key files are never
//! overwritten unless you choose `replace` (the old key is then kept next to it
//! under a `.replaced-<time>` name). Key paths point at this PC's `~/.ssh`.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, ValueEnum};
use githop_core::export::{
    self, ConflictKind, ImportAction, ImportConflict, ImportPreview, ImportResolution,
};
use githop_core::{EnforceMode, ProfileSet};

use super::export::{cli_error, count, read_password};
use super::{key_display, label_for, require_confirmation, Ctx};
use crate::exit::{usage, usage_hint, Exit};

/// What to do with a profile that clashes with one of yours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OnConflict {
    /// Keep yours, don't import theirs
    Skip,
    /// Replace yours with the one from the file
    Replace,
    /// Import it under a new ID
    Rename,
}

impl From<OnConflict> for ImportAction {
    fn from(c: OnConflict) -> Self {
        match c {
            OnConflict::Skip => ImportAction::Skip,
            OnConflict::Replace => ImportAction::Replace,
            OnConflict::Rename => ImportAction::Rename,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct ImportArgs {
    /// Export file to read, e.g. profiles.githop
    pub file: PathBuf,
    /// What to do with profiles that clash with yours (asked when not given)
    #[arg(long, value_enum, value_name = "ACTION")]
    pub on_conflict: Option<OnConflict>,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

pub fn run(ctx: &Ctx, args: ImportArgs) -> Result<Exit> {
    ctx.reject_json("import")?;
    let ui = &ctx.ui;
    let git = ctx.git()?;
    let store = ctx.store()?;
    if !args.file.is_file() {
        return Err(usage(format!(
            "There's no file at {}.",
            key_display(&args.file)
        )));
    }

    let password = read_password(ctx, false)?;
    let bundle = export::open_bundle(&args.file, &password).map_err(cli_error)?;
    drop(password);
    let preview = export::preview_bundle(store.paths(), git, &bundle).map_err(cli_error)?;
    let existing = store.profiles();

    show_preview(ctx, &preview, bundle.created());

    // Choose an action for every profile that needs one.
    let mut resolutions: Vec<ImportResolution> = Vec::new();
    let mut taken: Vec<String> = existing.profiles.iter().map(|p| p.id.clone()).collect();
    taken.extend(preview.profiles.iter().map(|p| p.id.clone()));
    for p in &preview.profiles {
        let conflicts: Vec<&ImportConflict> = preview
            .conflicts
            .iter()
            .filter(|c| c.profile_id == p.id)
            .collect();
        if !conflicts.iter().any(|c| c.kind.needs_choice()) {
            continue;
        }
        let resolution = match args.on_conflict {
            Some(choice) => ImportResolution {
                profile_id: p.id.clone(),
                action: choice.into(),
                new_id: None,
            },
            None if ui.interactive && !args.yes => ask(ctx, &p.id, &p.label, &mut taken)?,
            None => {
                let ids: Vec<String> = preview
                    .profiles
                    .iter()
                    .filter(|q| {
                        preview
                            .conflicts
                            .iter()
                            .any(|c| c.profile_id == q.id && c.kind.needs_choice())
                    })
                    .map(|q| q.id.clone())
                    .collect();
                return Err(cli_error(export::ExportError::Unresolved(ids)));
            }
        };
        resolutions.push(resolution);
    }

    let importing = preview
        .profiles
        .iter()
        .filter(|p| {
            !resolutions
                .iter()
                .any(|r| r.profile_id == p.id && r.action == ImportAction::Skip)
        })
        .count();
    if importing == 0 {
        ui.line("Nothing to import. Your profiles weren't changed.");
        return Ok(Exit::Ok);
    }
    let question = format!("Import {}?", count(importing, "profile"));
    if !require_confirmation(ctx, args.yes, &question, "Run again with --yes.")? {
        ui.line("Nothing was imported.");
        return Ok(Exit::Ok);
    }

    let before = store.profiles().clone();
    let settings_before = store.settings().clone();
    let report =
        export::apply_bundle(store.paths(), git, &bundle, &resolutions).map_err(cli_error)?;
    drop(bundle);
    let set = &report.profiles;

    ui.ok(&format!(
        "Imported {}: {}",
        count(report.imported.len(), "profile"),
        labels(set, &report.imported)
    ));
    for (from, to) in &report.renamed {
        ui.line(&format!("  {from} is now {to}"));
        // D23: a renamed profile whose key is identical keeps using that key.
        if let Some(p) = set.get(to) {
            let own = format!("githop_{to}");
            let name = p
                .ssh_key
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !name.is_empty() && name != own {
                ui.hint(&format!(
                    "  {to} uses your existing key {name} (same key), so no {own} was written."
                ));
            }
        }
    }
    if !report.replaced.is_empty() {
        ui.line(&format!("  Replaced: {}", report.replaced.join(", ")));
    }
    if !report.skipped.is_empty() {
        ui.line(&format!("  Skipped: {}", report.skipped.join(", ")));
    }
    for key in &report.keys_written {
        ui.ok(&format!(
            "SSH key saved: {} (only you can read it)",
            key_display(key)
        ));
    }
    for old in &report.keys_moved_aside {
        ui.hint(&format!(
            "Your old key file was kept as {}",
            key_display(old)
        ));
    }
    for note in &report.notes {
        ui.hint(note);
    }
    if report.settings_applied {
        let s = ctx.store()?;
        let mode = match s.settings().enforce {
            EnforceMode::Block => "block",
            EnforceMode::Warn => "warn",
            EnforceMode::BlockStrict => "block (strict)",
        };
        let logging = if s.settings().logging { "on" } else { "off" };
        ui.ok(&format!(
            "Settings copied: wrong identity → {mode}, activity log {logging}"
        ));
    }
    for (id, key) in &report.missing_keys {
        ui.warn(&format!(
            "{} has no SSH key on this PC yet ({}).",
            label_for(set, id),
            key_display(key)
        ));
        ui.hint(&format!(
            "Copy the key there, or create a new one: githop keygen {id}"
        ));
    }
    // D19: replaced profiles whose details changed, like `githop edit`.
    for id in &report.replaced {
        let (Some(old), Some(new)) = (before.get(id), set.get(id)) else {
            continue;
        };
        let changed = old.name != new.name
            || !old.email.eq_ignore_ascii_case(&new.email)
            || old.ssh_key != new.ssh_key;
        if !changed {
            continue;
        }
        if before.default.as_deref() == Some(id.as_str()) {
            ui.warn("Your global git identity still has the old details.");
            ui.hint(&format!("Update it: githop use {id}"));
        }
        let n = super::repos_pinned_to(git, &settings_before.pinned_repos, id).len();
        if n > 0 {
            ui.warn(&format!(
                "{} pinned to {} still {} the old details.",
                githop_core::text::plural(n, "repo", "repos"),
                new.label,
                if n == 1 { "uses" } else { "use" }
            ));
            ui.hint(&format!("Update them: githop edit {id} --repin"));
        }
    }
    if !settings_before.hooks_installed {
        // A new PC: nothing checks commits yet.
        ui.hint("Turn on the commit and push checks: githop init (or githop hooks install)");
    }
    if report.default_changed {
        if let Some(d) = set.default.as_deref() {
            ui.hint(&format!(
                "Default profile: {}. To use it for git everywhere, run: githop use {d}",
                label_for(set, d)
            ));
        }
    }
    Ok(Exit::Ok)
}

/// Comma-separated labels for ids.
fn labels(set: &ProfileSet, ids: &[String]) -> String {
    ids.iter()
        .map(|id| label_for(set, id))
        .collect::<Vec<_>>()
        .join(", ")
}

fn show_preview(ctx: &Ctx, preview: &ImportPreview, created: &str) {
    let ui = &ctx.ui;
    let when = created.split('T').next().unwrap_or(created);
    ui.line(&format!(
        "This file has {} (exported {when}):",
        count(preview.profiles.len(), "profile")
    ));
    let rows: Vec<Vec<String>> = preview
        .profiles
        .iter()
        .map(|p| {
            let name = p
                .ssh_key
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let key = if preview.keys.contains(&name) {
                "included"
            } else {
                "not included"
            };
            vec![
                p.id.clone(),
                p.label.clone(),
                p.email.clone(),
                key.to_string(),
            ]
        })
        .collect();
    ui.table(&["Id", "Label", "Email", "SSH key"], &rows);
    if preview.include_settings {
        ui.line(
            "Its settings (what happens on a wrong identity, activity log) will be copied too.",
        );
    }

    for p in &preview.profiles {
        let conflicts: Vec<&ImportConflict> = preview
            .conflicts
            .iter()
            .filter(|c| c.profile_id == p.id)
            .collect();
        if conflicts.is_empty() {
            continue;
        }
        ui.warn(&format!(
            "{} ({}) clashes with your profiles:",
            p.label, p.id
        ));
        for c in conflicts {
            ui.line(&format!("  • {}", describe(c, &p.ssh_key)));
        }
    }
}

fn describe(c: &ImportConflict, key: &std::path::Path) -> String {
    let other = &c.existing_id;
    match c.kind {
        ConflictKind::Id => format!("same ID as your profile {other}"),
        ConflictKind::Email => format!("same email as your profile {other}"),
        ConflictKind::Owner => format!("shares an owner with your profile {other}"),
        ConflictKind::Color => {
            format!("same colour as your profile {other} (it gets a free colour if needed)")
        }
        ConflictKind::KeyFile => {
            format!("a different SSH key already exists at {}", key_display(key))
        }
    }
}

/// Ask what to do with one clashing profile.
fn ask(ctx: &Ctx, id: &str, label: &str, taken: &mut Vec<String>) -> Result<ImportResolution> {
    let answer = ctx.ui.prompt(
        &format!("What should GitHop do with {label}? skip, replace or rename"),
        Some("skip"),
    )?;
    let action = match answer.to_ascii_lowercase().as_str() {
        "skip" | "s" => ImportAction::Skip,
        "replace" | "r" => ImportAction::Replace,
        "rename" | "n" => ImportAction::Rename,
        _ => {
            return Err(usage_hint(
                format!("\"{answer}\" isn't one of skip, replace or rename."),
                "Run the import again and pick one.",
            ))
        }
    };
    let new_id = if action == ImportAction::Rename {
        let suggestion = export::suggest_id(id, taken);
        let chosen = ctx.ui.prompt("New profile ID", Some(&suggestion))?;
        taken.push(chosen.clone());
        Some(chosen)
    } else {
        None
    };
    Ok(ImportResolution {
        profile_id: id.to_string(),
        action,
        new_id,
    })
}
