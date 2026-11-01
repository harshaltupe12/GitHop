//! Serde DTOs shared with the UI (`app/IPC.md`, mirrored in `app/ui/src/lib/types.ts`).
//!
//! Field names are snake_case exactly as written here. Core types (`Profile`,
//! `ProfileSet`, `Settings`, `HookHealth`, `ScannedRepo`, …) are re-used as-is.

use std::io::ErrorKind;

use githop_core::export::ExportError;
use githop_core::hooks::HookHealth;
use githop_core::scan::ScannedRepo;
use githop_core::{CoreError, EnforceMode, Identity, ProfileSet, Settings, ValidationIssue};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Errors

/// Every command's error type. `issues` is non-empty only for `validation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcError {
    pub code: ErrorCode,
    /// User-facing, sentence case (Design §9 voice).
    pub message: String,
    pub issues: Vec<ValidationIssue>,
}

/// Machine-readable error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Validation,
    Locked,
    Conflict,
    NotFound,
    NotARepo,
    Git,
    Io,
    Unsupported,
    BadInput,
    Internal,
}

impl IpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: sentence(&message.into()),
            issues: Vec::new(),
        }
    }

    pub fn bad_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadInput, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// A feature whose core API isn't wired in yet (see `// INTEGRATE:` comments).
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }

    /// Validation failure carrying every issue (errors, warnings, info).
    pub fn validation(issues: Vec<ValidationIssue>) -> Self {
        let first = issues
            .iter()
            .find(|i| i.severity == githop_core::Severity::Error)
            .or_else(|| issues.first())
            .map(|i| i.message.clone())
            .unwrap_or_else(|| "Some details need fixing.".to_string());
        Self {
            code: ErrorCode::Validation,
            message: sentence(&first),
            issues,
        }
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<ExportError> for IpcError {
    fn from(e: ExportError) -> Self {
        match e {
            ExportError::Core(core) => core.into(),
            ExportError::InvalidProfiles(issues) => IpcError::validation(issues),
            ExportError::Conflict(msg) => IpcError::conflict(msg),
            ExportError::NoHome => IpcError::internal(e.to_string()),
            other if other.is_user_error() => IpcError::bad_input(other.to_string()),
            other => IpcError::internal(other.to_string()),
        }
    }
}

impl From<CoreError> for IpcError {
    fn from(e: CoreError) -> Self {
        let message = e.to_string();
        match e {
            CoreError::Validation(issues) => IpcError::validation(issues),
            CoreError::Io { ref source, .. } => match source.kind() {
                ErrorKind::NotFound => IpcError::new(ErrorCode::NotFound, message),
                ErrorKind::AlreadyExists => IpcError::new(ErrorCode::Conflict, message),
                _ => IpcError::new(ErrorCode::Io, message),
            },
            CoreError::Toml { .. } | CoreError::BackupFailed(_) => {
                IpcError::new(ErrorCode::Io, message)
            }
            CoreError::UnsupportedVersion { .. } => IpcError::new(ErrorCode::Unsupported, message),
            CoreError::GitNotFound | CoreError::GitFailed { .. } => {
                IpcError::new(ErrorCode::Git, message)
            }
            CoreError::NotARepo => IpcError::new(ErrorCode::NotARepo, message),
            CoreError::Locked => IpcError::new(ErrorCode::Locked, message),
            CoreError::Conflict { .. } => IpcError::new(ErrorCode::Conflict, message),
            // Already a full user-facing sentence with the next step.
            CoreError::FixRefused(msg) => IpcError::new(ErrorCode::BadInput, msg),
            CoreError::NoDataDir => IpcError::new(ErrorCode::Internal, message),
        }
    }
}

/// Upper-case the first letter so core's lower-case messages read as sentences.
pub fn sentence(s: &str) -> String {
    let s = s.trim();
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Result alias for command bodies.
pub type IpcResult<T> = Result<T, IpcError>;

// ---------------------------------------------------------------------------
// App-only DTOs

/// Where onboarding should resume (Design §8.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingState {
    pub has_profiles: bool,
    /// Hooks installed **and** global `core.hooksPath` points at them.
    pub hooks_installed: bool,
    /// Current global `user.name` + `user.email`, when both are set.
    pub global_identity: Option<Identity>,
    /// `"welcome"` (nothing done yet) · `"first-profile"` (backup made, no profile)
    /// · `"protection"` (profiles exist, hooks not installed) · `"done"`.
    pub step_hint: String,
    /// Where GitHop keeps its backups (forward slashes), shown in "Safety first".
    pub backup_dir: String,
}

/// Everything the main window needs on start and on focus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    pub profiles: ProfileSet,
    pub settings: Settings,
    pub hooks: HookHealth,
    pub onboarding: OnboardingState,
    pub autostart: bool,
}

/// Partial settings update; `None` = leave as is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsPatch {
    #[serde(default)]
    pub enforce: Option<EnforceMode>,
    #[serde(default)]
    pub logging: Option<bool>,
    #[serde(default)]
    pub autostart: Option<bool>,
}

/// About box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub licenses: String,
}

/// `scan://progress` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    pub folder: String,
    pub found: usize,
    pub done: usize,
}

/// `ScannedRepo` (core) plus the app-side count of unpushed wrong-identity
/// commits (`fix::plan(..).offenders.len()`, 0 when unresolved/blocked/error),
/// so the Repos screen needs no `fix_plan` per row. Serialises flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRow {
    #[serde(flatten)]
    pub repo: ScannedRepo,
    pub unpushed_offenders: usize,
}

// ---------------------------------------------------------------------------
// Core types re-exported for the IPC layer (shapes in app/IPC.md).

pub use githop_core::doctor::DoctorReport;
pub use githop_core::wipe::{WipeOptions, WipeReport};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn error_serialises_with_snake_case_code() {
        let e = IpcError::new(ErrorCode::NotARepo, "this folder is not a git repo");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "not_a_repo");
        assert_eq!(v["message"], "This folder is not a git repo");
        assert_eq!(v["issues"], serde_json::json!([]));
        let v = serde_json::to_value(IpcError::bad_input("x")).unwrap();
        assert_eq!(v["code"], "bad_input");
    }

    #[test]
    fn core_errors_map_to_codes() {
        let cases: Vec<(CoreError, ErrorCode)> = vec![
            (CoreError::Locked, ErrorCode::Locked),
            (CoreError::NotARepo, ErrorCode::NotARepo),
            (CoreError::GitNotFound, ErrorCode::Git),
            (
                CoreError::io(
                    PathBuf::from("x"),
                    std::io::Error::new(ErrorKind::NotFound, "gone"),
                ),
                ErrorCode::NotFound,
            ),
            (
                CoreError::io(
                    PathBuf::from("x"),
                    std::io::Error::new(ErrorKind::AlreadyExists, "there"),
                ),
                ErrorCode::Conflict,
            ),
            (
                CoreError::io(PathBuf::from("x"), std::io::Error::other("boom")),
                ErrorCode::Io,
            ),
            (CoreError::NoDataDir, ErrorCode::Internal),
            (
                CoreError::Conflict {
                    path: PathBuf::from("profiles.toml"),
                },
                ErrorCode::Conflict,
            ),
        ];
        for (core, code) in cases {
            assert_eq!(IpcError::from(core).code, code);
        }
    }

    #[test]
    fn fix_refusal_is_bad_input_with_cores_sentence() {
        let msg = "Finish or abort the rebase first, then run fix again.";
        let e = IpcError::from(CoreError::FixRefused(msg.into()));
        assert_eq!(e.code, ErrorCode::BadInput);
        assert_eq!(e.message, msg);
    }

    #[test]
    fn validation_keeps_all_issues_and_leads_with_an_error() {
        let issues = vec![
            ValidationIssue {
                profile_id: Some("a".into()),
                field: "email".into(),
                severity: githop_core::Severity::Warning,
                message: "Use a noreply email.".into(),
            },
            ValidationIssue {
                profile_id: Some("a".into()),
                field: "id".into(),
                severity: githop_core::Severity::Error,
                message: "Id is taken.".into(),
            },
        ];
        let e = IpcError::from(CoreError::Validation(issues.clone()));
        assert_eq!(e.code, ErrorCode::Validation);
        assert_eq!(e.message, "Id is taken.");
        assert_eq!(e.issues, issues);
    }

    #[test]
    fn repo_row_serialises_flat() {
        let row = RepoRow {
            repo: ScannedRepo {
                root: "C:/code/a".into(),
                resolution: githop_core::Resolution {
                    profile_id: None,
                    source: None,
                    remote_name: None,
                    remote: None,
                    pinned: None,
                    warnings: vec![],
                },
                identity: None,
                error: None,
            },
            unpushed_offenders: 2,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["root"], "C:/code/a");
        assert_eq!(v["unpushed_offenders"], 2);
        assert!(v.get("repo").is_none());
    }

    #[test]
    fn export_errors_map_to_codes() {
        assert_eq!(
            IpcError::from(ExportError::WrongPassword).code,
            ErrorCode::BadInput
        );
        assert_eq!(
            IpcError::from(ExportError::Conflict("taken".into())).code,
            ErrorCode::Conflict
        );
        assert_eq!(
            IpcError::from(ExportError::InvalidProfiles(vec![])).code,
            ErrorCode::Validation
        );
        assert_eq!(
            IpcError::from(ExportError::Core(CoreError::Locked)).code,
            ErrorCode::Locked
        );
        assert_eq!(
            IpcError::from(ExportError::NoHome).code,
            ErrorCode::Internal
        );
    }

    #[test]
    fn core_dtos_match_contract() {
        let d = githop_core::doctor::DoctorCheck {
            id: "hooks".into(),
            status: githop_core::doctor::CheckStatus::Problem,
            title: "t".into(),
            detail: "d".into(),
            fix: None,
        };
        let v = serde_json::to_value(d).unwrap();
        assert_eq!(v["status"], "problem");
        assert!(v.get("fix").is_none());
        let p: SettingsPatch = serde_json::from_str(r#"{"enforce":"block-strict"}"#).unwrap();
        assert_eq!(p.enforce, Some(EnforceMode::BlockStrict));
        assert_eq!(p.autostart, None);
    }
}
