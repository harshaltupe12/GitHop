//! `githop fix [--yes] [--dry-run] [--undo [<backup-ref>]]` — re-stamp unpushed
//! commits that used the wrong identity (Phase 4; Plan §7.5; Design §8.4, §9.3).
//!
//! Always says that only unpushed commits are changed and that a backup is
//! saved first. Pushed commits with the wrong email are listed but never touched.
//!
//! Exit codes: `0` fixed / undone / nothing to fix · `1` something to fix that
//! wasn't fixed (dry run, declined, blocked) · `2` usage · `3` internal error.
//!
//! JSON (`--json`): `--dry-run` prints the `FixPlan`; `--yes` prints
//! `{ "plan": FixPlan, "result": FixResult | null }`; `--undo` prints
//! `{ "undone": "<backup ref>" }`. Without one of those flags `--json` is a
//! usage error (there'd be no way to confirm).

use anyhow::Result;
use githop_core::check::OffendingCommit;
use githop_core::fix::{self, FixPlan};
use githop_core::{CoreError, Paths, Profile, ProfileSet, Store};
use serde_json::json;

use super::{cwd, require_confirmation, Ctx};
use crate::cli::FixArgs;
use crate::exit::{usage_hint, Exit};

const REASSURANCE: &str =
    "Only unpushed commits are changed. A backup is saved first, so you can undo.";

pub fn run(ctx: &Ctx, args: FixArgs) -> Result<Exit> {
    if let Some(given) = args.undo.as_deref() {
        return run_undo(ctx, &args, given);
    }
    let ui = &ctx.ui;
    if ui.json && !args.yes && !args.dry_run {
        return Err(usage_hint(
            "githop fix --json needs --dry-run or --yes.",
            "Run: githop fix --json --dry-run",
        ));
    }
    let git = ctx.git()?;
    let dir = cwd()?;
    let paths = Paths::resolve()?;
    let plan = fix::plan(git, &dir, &paths)?;
    let profiles = Store::open_at(paths.clone())
        .map(|s| s.profiles().clone())
        .unwrap_or_default();
    let owner = plan
        .profile_id
        .as_deref()
        .and_then(|id| profiles.get(id))
        .cloned();
    let pending = if plan.blocked_reason.is_some() || !plan.offenders.is_empty() {
        Exit::Blocked
    } else {
        Exit::Ok
    };

    if ui.json && args.dry_run {
        ui.json_out(&serde_json::to_value(&plan)?)?;
        return Ok(pending);
    }
    if !ui.json {
        show_plan(ctx, &plan, &profiles, owner.as_ref());
    }
    if !plan.can_apply() {
        if ui.json {
            if let Some(reason) = &plan.blocked_reason {
                ui.error(reason);
            }
            ui.json_out(&json!({ "plan": plan, "result": null }))?;
        }
        return Ok(pending);
    }
    let Some(owner) = owner else {
        return Ok(Exit::Blocked);
    };
    if args.dry_run {
        ui.hint("Run: githop fix");
        return Ok(Exit::Blocked);
    }
    let question = format!(
        "Fix {} with the {} identity ({} <{}>)?",
        commits(plan.offenders.len()),
        owner.label,
        owner.name,
        owner.email
    );
    if !require_confirmation(ctx, args.yes, &question, "Run: githop fix --yes")? {
        ui.line("Nothing was changed.");
        return Ok(Exit::Blocked);
    }

    let result = match fix::apply(git, &dir, &paths) {
        Ok(r) => r,
        Err(CoreError::FixRefused(msg)) => {
            refuse(ctx, &msg);
            if ui.json {
                ui.json_out(&json!({ "plan": plan, "result": null }))?;
            }
            return Ok(Exit::Blocked);
        }
        Err(e) => return Err(e.into()),
    };
    if ui.json {
        ui.json_out(&json!({ "plan": plan, "result": result }))?;
        return Ok(Exit::Ok);
    }
    if result.fast_forward {
        ui.ok(&format!(
            "{} fixed. You can push now.",
            commits(result.rewritten)
        ));
    } else {
        ui.ok(&format!("{} fixed.", commits(result.rewritten)));
        let branch = plan.branch.as_deref().unwrap_or("This branch");
        ui.warn(&format!(
            "{branch} and its remote have both changed, so a plain push will be rejected."
        ));
        ui.hint("  Run: git pull --rebase, then git push");
    }
    ui.line(&format!("  Backup: {}", result.backup_ref));
    ui.hint("  Undo: githop fix --undo");
    Ok(Exit::Ok)
}

fn run_undo(ctx: &Ctx, args: &FixArgs, given: &str) -> Result<Exit> {
    let ui = &ctx.ui;
    let git = ctx.git()?;
    let dir = cwd()?;
    let backup = if given.trim().is_empty() {
        match fix::latest_backup(git, &dir)? {
            Some(b) => b,
            None => {
                refuse(ctx, "There's no fix to undo on this branch.");
                if ui.json {
                    ui.json_out(&json!({ "undone": null }))?;
                }
                return Ok(Exit::Blocked);
            }
        }
    } else {
        given.trim().to_string()
    };
    if !ui.json {
        ui.line(&format!("This restores {backup}."));
    }
    if !require_confirmation(
        ctx,
        args.yes,
        "Undo the fix?",
        "Run: githop fix --undo --yes",
    )? {
        ui.line("Nothing was changed.");
        return Ok(Exit::Blocked);
    }
    match fix::undo(git, &dir, &backup) {
        Ok(()) => {
            if ui.json {
                ui.json_out(&json!({ "undone": backup }))?;
            } else {
                ui.ok("Fix undone. Your branch is back where it was before the fix.");
            }
            Ok(Exit::Ok)
        }
        Err(CoreError::FixRefused(msg)) => {
            refuse(ctx, &msg);
            if ui.json {
                ui.json_out(&json!({ "undone": null }))?;
            }
            Ok(Exit::Blocked)
        }
        Err(e) => Err(e.into()),
    }
}

/// `✗ msg` on stdout for people, on stderr with `--json` (stdout stays JSON).
fn refuse(ctx: &Ctx, msg: &str) {
    if ctx.ui.json {
        ctx.ui.error(msg);
    } else {
        ctx.ui.blocked(msg);
    }
}

fn show_plan(ctx: &Ctx, plan: &FixPlan, profiles: &ProfileSet, owner: Option<&Profile>) {
    let ui = &ctx.ui;
    let arrow = if ui.ascii { "->" } else { "→" };
    let label = owner.map(|p| p.label.clone()).unwrap_or_default();
    let right = plan.profile_email.clone().unwrap_or_default();

    if !plan.pushed_offenders.is_empty() {
        let n = plan.pushed_offenders.len();
        ui.warn(&format!(
            "{} already pushed {} another profile's email. GitHop never changes pushed commits.",
            commits(n),
            verb(n, "uses", "use")
        ));
        ui.table(
            &["Commit", "Message", "Email"],
            &rows(&plan.pushed_offenders, None, arrow),
        );
        ui.hint(
            "To change them you'd have to rewrite shared history yourself (git rebase, then a \
             force push). Check with your team first.",
        );
    }

    if !plan.offenders.is_empty() {
        let n = plan.offenders.len();
        let headline = match other_profile(plan, profiles) {
            Some(other) => format!(
                "{} {} the {} email in this {label} repo.",
                unpushed(n),
                verb(n, "uses", "use"),
                other.label
            ),
            None => format!(
                "{} {} the {label} email.",
                unpushed(n),
                verb(n, "doesn't use", "don't use")
            ),
        };
        ui.warn(&headline);
        ui.table(
            &["Commit", "Message", "Email"],
            &rows(&plan.offenders, Some(&right), arrow),
        );
        let later = plan.rewrite_count.saturating_sub(n);
        if later > 0 {
            ui.line(&format!(
                "{} after them {} new ids too (the content doesn't change).",
                commits(later),
                verb(later, "gets", "get")
            ));
        }
        if plan.foreign > 0 {
            ui.line(&if plan.foreign == 1 {
                "1 commit by someone else keeps its author.".to_string()
            } else {
                format!(
                    "{} commits by other people keep their authors.",
                    plan.foreign
                )
            });
        }
        if !plan.has_remote_refs {
            ui.warn(&format!(
                "This repo has no remote, so all {} {} checked.",
                commits(plan.unpushed),
                verb(plan.unpushed, "was", "were")
            ));
        }
        if plan.signed > 0 {
            ui.warn(&format!(
                "{} signed. Fixing removes the signatures.",
                if plan.signed == 1 {
                    "1 of these commits is".to_string()
                } else {
                    format!("{} of these commits are", plan.signed)
                }
            ));
        }
        if plan.blocked_reason.is_none() {
            ui.line(REASSURANCE);
        }
    }

    if let Some(reason) = &plan.blocked_reason {
        ui.blocked(reason);
    } else if plan.offenders.is_empty() {
        match &plan.branch {
            Some(branch) if !label.is_empty() => ui.ok(&format!(
                "Nothing to fix. All unpushed commits on {branch} use the {label} email."
            )),
            _ => ui.ok("Nothing to fix."),
        }
    }
}

/// Table rows: short hash, subject, wrong email (→ right email).
fn rows(list: &[OffendingCommit], right: Option<&str>, arrow: &str) -> Vec<Vec<String>> {
    list.iter()
        .map(|o| {
            let email = match right {
                Some(r) => format!("{} {arrow} {r}", o.wrong_email()),
                None => o.wrong_email().to_string(),
            };
            vec![o.sha.chars().take(7).collect(), o.subject.clone(), email]
        })
        .collect()
}

/// The one other profile whose email every offender uses, if there is one.
fn other_profile<'a>(plan: &FixPlan, profiles: &'a ProfileSet) -> Option<&'a Profile> {
    let first = plan.offenders.first()?.wrong_email().trim().to_lowercase();
    if plan
        .offenders
        .iter()
        .any(|o| o.wrong_email().trim().to_lowercase() != first)
    {
        return None;
    }
    let other = profiles
        .profiles
        .iter()
        .find(|p| p.email.trim().to_lowercase() == first)?;
    (Some(other.id.as_str()) != plan.profile_id.as_deref()).then_some(other)
}

fn commits(n: usize) -> String {
    if n == 1 {
        "1 commit".to_string()
    } else {
        format!("{n} commits")
    }
}

fn unpushed(n: usize) -> String {
    if n == 1 {
        "1 unpushed commit".to_string()
    } else {
        format!("{n} unpushed commits")
    }
}

fn verb(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_verbs() {
        assert_eq!(commits(1), "1 commit");
        assert_eq!(commits(3), "3 commits");
        assert_eq!(unpushed(2), "2 unpushed commits");
        assert_eq!(verb(1, "uses", "use"), "uses");
        assert_eq!(verb(2, "uses", "use"), "use");
    }

    #[test]
    fn rows_show_wrong_and_right_email() {
        let o = OffendingCommit {
            sha: "abcdef1234567890".into(),
            subject: "add billing".into(),
            author_email: "me@personal.dev".into(),
            committer_email: "me@personal.dev".into(),
            author_mismatch: true,
            committer_mismatch: true,
        };
        let r = rows(&[o], Some("me@company.com"), "→");
        assert_eq!(
            r[0],
            vec![
                "abcdef1".to_string(),
                "add billing".to_string(),
                "me@personal.dev → me@company.com".to_string()
            ]
        );
    }
}
