//! `githop doctor [--json]` — render the core diagnosis (`githop_core::doctor`).
//!
//! Exit code: `1` when there is at least one problem, else `0`.

use anyhow::Result;
use githop_core::doctor::{self, CheckStatus, DoctorCheck, DoctorOptions, DoctorReport};
use githop_core::Paths;
use serde_json::json;

use super::Ctx;
use crate::exit::Exit;

/// How a check is grouped in human output. `ok` + `fix` = a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Passing,
    Notes,
    Warnings,
    Problems,
}

fn group_of(c: &DoctorCheck) -> Group {
    match (c.status, c.fix.is_some()) {
        (CheckStatus::Ok, false) => Group::Passing,
        (CheckStatus::Ok, true) => Group::Notes,
        (CheckStatus::Warning, _) => Group::Warnings,
        (CheckStatus::Problem, _) => Group::Problems,
    }
}

pub fn run(ctx: &Ctx) -> Result<Exit> {
    let paths = Paths::resolve()?;
    let opts = DoctorOptions {
        cwd: std::env::current_dir().ok(),
        current_exe: super::hooks::current_exe().ok(),
        refresh_aliases: true,
    };
    let report = doctor::diagnose(&paths, ctx.git().ok(), &opts);
    let code = if report.has_problems() {
        Exit::Blocked
    } else {
        Exit::Ok
    };
    if ctx.ui.json {
        ctx.ui.json_out(&json!({
            "checks": report.checks,
            "summary": {
                "ok": report.count(CheckStatus::Ok),
                "warning": report.count(CheckStatus::Warning),
                "problem": report.count(CheckStatus::Problem),
            },
        }))?;
        return Ok(code);
    }
    print_report(ctx, &report);
    Ok(code)
}

fn print_report(ctx: &Ctx, r: &DoctorReport) {
    let ui = &ctx.ui;
    let groups = [
        (Group::Passing, "Passing"),
        (Group::Notes, "Notes"),
        (Group::Warnings, "Warnings"),
        (Group::Problems, "Problems"),
    ];
    let mut first = true;
    for (group, title) in groups {
        let items: Vec<&DoctorCheck> = r.checks.iter().filter(|c| group_of(c) == group).collect();
        if items.is_empty() || (group == Group::Passing && ui.quiet) {
            continue;
        }
        if !first {
            ui.line("");
        }
        first = false;
        if ui.quiet {
            println!("{title}");
        } else {
            ui.line(title);
        }
        for c in items {
            let msg = format!("{}: {}", c.title, c.detail);
            match group {
                Group::Passing => ui.ok(&msg),
                Group::Notes => ui.line(&format!("{} {msg}", if ui.ascii { "-" } else { "·" })),
                Group::Warnings => ui.warn(&msg),
                Group::Problems => ui.blocked(&msg),
            }
            if group != Group::Passing {
                if let Some(fix) = &c.fix {
                    ui.hint(&format!("  Fix: {fix}"));
                }
            }
        }
    }
    ui.line("");
    let problems = r.count(CheckStatus::Problem);
    let warnings = r.count(CheckStatus::Warning);
    if problems == 0 && warnings == 0 {
        ui.ok("Everything looks good.");
    } else {
        let text = format!("{problems} problem(s), {warnings} warning(s).");
        if problems > 0 {
            ui.blocked(&text);
        } else {
            ui.warn(&text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: CheckStatus, fix: Option<&str>) -> DoctorCheck {
        DoctorCheck {
            id: "x".into(),
            status,
            title: "T".into(),
            detail: "d".into(),
            fix: fix.map(str::to_string),
        }
    }

    #[test]
    fn grouping() {
        assert_eq!(group_of(&check(CheckStatus::Ok, None)), Group::Passing);
        assert_eq!(group_of(&check(CheckStatus::Ok, Some("f"))), Group::Notes);
        assert_eq!(group_of(&check(CheckStatus::Warning, None)), Group::Warnings);
        assert_eq!(
            group_of(&check(CheckStatus::Problem, Some("f"))),
            Group::Problems
        );
    }
}
