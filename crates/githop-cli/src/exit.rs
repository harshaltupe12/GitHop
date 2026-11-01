//! Exit codes and error reporting (CLAUDE.md conventions).
//!
//! `0` ok · `1` blocked / a check found a problem · `2` usage or validation error ·
//! `3` internal error (hooks treat it as fail-open).

use std::fmt;

use githop_core::{CoreError, Severity, ValidationIssue};

use crate::ui::Ui;

/// Process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok = 0,
    Blocked = 1,
    Usage = 2,
    Internal = 3,
}

impl From<Exit> for std::process::ExitCode {
    fn from(e: Exit) -> Self {
        std::process::ExitCode::from(e as u8)
    }
}

/// A mistake in how the command was called (exit 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError {
    pub message: String,
    /// Bulleted detail lines (e.g. missing fields).
    pub details: Vec<String>,
    /// One next step, e.g. `Run: githop list`.
    pub hint: Option<String>,
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UsageError {}

/// Build a usage error with just a message.
pub fn usage(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(UsageError {
        message: message.into(),
        details: Vec::new(),
        hint: None,
    })
}

/// Build a usage error with a next-step hint.
pub fn usage_hint(message: impl Into<String>, hint: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(UsageError {
        message: message.into(),
        details: Vec::new(),
        hint: Some(hint.into()),
    })
}

/// Required values that weren't given and can't be asked for (stdin not a terminal).
pub fn missing_fields(fields: &[&str]) -> anyhow::Error {
    anyhow::Error::new(UsageError {
        message: "Some required values are missing:".to_string(),
        details: fields.iter().map(|f| f.to_string()).collect(),
        hint: Some("Pass them as flags, or run this in a terminal to be asked.".to_string()),
    })
}

/// Upper-case the first letter (core messages start lower-case).
pub fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Bullet lines for validation issues: errors first, then warnings and info.
pub fn issue_lines(issues: &[ValidationIssue]) -> Vec<String> {
    let mut sorted: Vec<&ValidationIssue> = issues.iter().collect();
    sorted.sort_by_key(|i| match i.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    });
    sorted
        .into_iter()
        .map(|i| {
            let tag = match i.severity {
                Severity::Error => "",
                Severity::Warning => " (warning)",
                Severity::Info => " (note)",
            };
            let who = i
                .profile_id
                .as_deref()
                .map(|p| format!("{p}: "))
                .unwrap_or_default();
            format!("  • {who}{}{tag} — {}", i.field, i.message)
        })
        .collect()
}

/// Print `err` for the user and pick the exit code.
pub fn report(ui: &Ui, err: &anyhow::Error) -> Exit {
    if let Some(u) = err.downcast_ref::<UsageError>() {
        ui.error(&u.message);
        for d in &u.details {
            ui.error_line(&format!("  • {d}"));
        }
        if let Some(h) = &u.hint {
            ui.error_hint(h);
        }
        return Exit::Usage;
    }
    if let Some(core) = err.downcast_ref::<CoreError>() {
        return report_core(ui, err, core);
    }
    ui.error(&format!(
        "GitHop hit a problem: {}",
        sentence(&format!("{err:#}"))
    ));
    ui.error_hint("Run: githop doctor");
    Exit::Internal
}

fn report_core(ui: &Ui, err: &anyhow::Error, core: &CoreError) -> Exit {
    match core {
        CoreError::Validation(issues) => {
            ui.error("The profile isn't valid. Nothing was saved.");
            for line in issue_lines(issues) {
                ui.error_line(&line);
            }
            Exit::Usage
        }
        CoreError::NotARepo => {
            ui.error("This folder is not a git repo.");
            ui.error_hint("Run this inside a repo, or use: githop scan <folder>");
            Exit::Usage
        }
        CoreError::GitNotFound | CoreError::Locked | CoreError::NoDataDir => {
            ui.error(&sentence(&core.to_string()));
            Exit::Internal
        }
        CoreError::UnsupportedVersion { .. } => {
            ui.error(&sentence(&core.to_string()));
            ui.error_hint("Update GitHop to read this file.");
            Exit::Internal
        }
        CoreError::Conflict { .. } => {
            // Another GitHop window/command changed the file first; nothing saved.
            ui.error(&sentence(&core.to_string()));
            ui.error_hint("Run the same command again.");
            Exit::Internal
        }
        CoreError::FixRefused(reason) => {
            // Fix refused on purpose (pushed commits, dirty tree…): nothing changed.
            ui.error(&sentence(reason));
            ui.error_hint("Nothing was changed. Run: githop fix --help");
            Exit::Blocked
        }
        _ => {
            ui.error(&format!(
                "GitHop hit a problem: {}",
                sentence(&format!("{err:#}"))
            ));
            ui.error_hint("Run: githop doctor");
            Exit::Internal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(Exit::Ok as u8, 0);
        assert_eq!(Exit::Blocked as u8, 1);
        assert_eq!(Exit::Usage as u8, 2);
        assert_eq!(Exit::Internal as u8, 3);
    }

    #[test]
    fn usage_and_core_errors_map_to_codes() {
        let ui = Ui::plain();
        assert_eq!(report(&ui, &usage("bad")), Exit::Usage);
        assert_eq!(report(&ui, &missing_fields(&["--email"])), Exit::Usage);
        assert_eq!(
            report(&ui, &anyhow::Error::new(CoreError::NotARepo)),
            Exit::Usage
        );
        assert_eq!(
            report(&ui, &anyhow::Error::new(CoreError::Validation(vec![]))),
            Exit::Usage
        );
        assert_eq!(
            report(&ui, &anyhow::Error::new(CoreError::Locked)),
            Exit::Internal
        );
        assert_eq!(report(&ui, &anyhow::anyhow!("boom")), Exit::Internal);
        assert_eq!(
            report(
                &ui,
                &anyhow::Error::new(CoreError::Conflict {
                    path: "profiles.toml".into()
                })
            ),
            Exit::Internal
        );
        assert_eq!(
            report(&ui, &anyhow::Error::new(CoreError::FixRefused("x".into()))),
            Exit::Blocked
        );
        let ctx = anyhow::Error::new(CoreError::NotARepo).context("while pinning");
        assert_eq!(report(&ui, &ctx), Exit::Usage);
    }

    #[test]
    fn sentence_capitalises() {
        assert_eq!(sentence("couldn't read x"), "Couldn't read x");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn issues_errors_first() {
        let issues = vec![
            ValidationIssue {
                profile_id: Some("personal".into()),
                field: "email".into(),
                severity: Severity::Warning,
                message: "Use a noreply email".into(),
            },
            ValidationIssue {
                profile_id: Some("personal".into()),
                field: "id".into(),
                severity: Severity::Error,
                message: "Id is taken".into(),
            },
        ];
        let lines = issue_lines(&issues);
        assert_eq!(lines[0], "  • personal: id — Id is taken");
        assert_eq!(
            lines[1],
            "  • personal: email (warning) — Use a noreply email"
        );
    }
}
