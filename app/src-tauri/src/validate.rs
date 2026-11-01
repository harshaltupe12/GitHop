//! Input validation for every IPC command (Plan §9.2: never trust the UI).
//!
//! Pure functions, no I/O except the existence checks, so they are unit-tested
//! directly without a WebView.

use std::path::{Component, Path, PathBuf};

use githop_core::{Profile, Severity, ValidationIssue};

use crate::dto::{ImportAction, ImportResolution, IpcError, IpcResult};

/// Longest path GitHop accepts from the UI.
const MAX_PATH_LEN: usize = 4096;
/// Longest single text field (label, name, email, owner, folder).
const MAX_TEXT_LEN: usize = 256;
/// Most owners / folders per profile.
const MAX_LIST_LEN: usize = 100;
/// Export password rules (Phase 6 §3).
pub const MIN_PASSWORD_CHARS: usize = 12;
const MAX_PASSWORD_CHARS: usize = 1024;
/// Most repos accepted by one `pin_all` call.
pub const MAX_PIN_ALL: usize = 5000;

/// `^[a-z0-9-]{1,32}$` (same rule as core's profile id).
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A profile id argument.
pub fn profile_id(id: &str) -> IpcResult<&str> {
    if is_valid_id(id) {
        Ok(id)
    } else {
        Err(IpcError::bad_input(format!(
            "\"{}\" isn't a valid profile id. Use 1–32 lower-case letters, numbers or dashes.",
            printable(id)
        )))
    }
}

/// An absolute local path without `..`, NUL or control characters.
/// Device namespaces (`\\?\`, `\\.\`) are refused.
pub fn abs_path(s: &str) -> IpcResult<PathBuf> {
    if s.trim().is_empty() {
        return Err(IpcError::bad_input("Choose a folder or file first."));
    }
    if s.len() > MAX_PATH_LEN {
        return Err(IpcError::bad_input("That path is too long."));
    }
    if s.chars().any(char::is_control) {
        return Err(IpcError::bad_input(
            "That path contains invalid characters.",
        ));
    }
    let slashed = s.replace('\\', "/");
    if slashed.starts_with("//?/") || slashed.starts_with("//./") {
        return Err(IpcError::bad_input("Device paths aren't supported."));
    }
    let path = PathBuf::from(s);
    // `githop_core::paths::is_absolute_str` accepts `/x` on Windows too; also
    // require the platform's own notion so `C:x` (drive-relative) is refused.
    if !githop_core::paths::is_absolute_str(s) || !path.is_absolute() {
        return Err(IpcError::bad_input(format!(
            "{} isn't a full path. Choose it with the folder picker.",
            printable(s)
        )));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(IpcError::bad_input("Paths with \"..\" aren't allowed."));
    }
    Ok(path)
}

/// An absolute path to an existing folder.
pub fn existing_dir(s: &str) -> IpcResult<PathBuf> {
    let p = abs_path(s)?;
    if p.is_dir() {
        Ok(p)
    } else {
        Err(IpcError::not_found(format!(
            "There's no folder at {}.",
            printable(s)
        )))
    }
}

/// An absolute path to an existing file.
pub fn existing_file(s: &str) -> IpcResult<PathBuf> {
    let p = abs_path(s)?;
    if p.is_file() {
        Ok(p)
    } else {
        Err(IpcError::not_found(format!(
            "There's no file at {}.",
            printable(s)
        )))
    }
}

/// An absolute path to a file that may not exist yet, in an existing folder.
pub fn new_file_in_existing_dir(s: &str) -> IpcResult<PathBuf> {
    let p = abs_path(s)?;
    if p.file_name().is_none() || s.ends_with(['/', '\\']) {
        return Err(IpcError::bad_input("Choose a file name, not a folder."));
    }
    if p.is_dir() {
        return Err(IpcError::bad_input("Choose a file name, not a folder."));
    }
    match p.parent() {
        Some(parent) if parent.is_dir() => Ok(p),
        _ => Err(IpcError::not_found(format!(
            "The folder for {} doesn't exist.",
            printable(s)
        ))),
    }
}

/// Hook stage for `hooks_snippet`.
pub fn stage(s: &str) -> IpcResult<&'static str> {
    match s {
        "commit" => Ok("commit"),
        "push" => Ok("push"),
        "clone" => Ok("clone"),
        other => Err(IpcError::bad_input(format!(
            "\"{}\" isn't a hook stage. Use commit, push or clone.",
            printable(other)
        ))),
    }
}

/// A fix backup ref (`refs/githop/backup/<branch>/<time>`). Rules from
/// `git check-ref-format`: no control chars, whitespace, `~^:?*[\`, `..`,
/// `@{`, `//`, parts starting with `.`/`-` or ending in `.lock`. Core's
/// `fix::undo` then checks it really is one of its backups.
pub fn backup_ref(s: &str) -> IpcResult<&str> {
    let bad = || IpcError::bad_input("That backup reference isn't valid.");
    let Some(rest) = s.strip_prefix(githop_core::fix::BACKUP_PREFIX) else {
        return Err(bad());
    };
    if rest.is_empty() || s.len() > 1024 || s.ends_with('/') || s.ends_with('.') {
        return Err(bad());
    }
    if s.chars()
        .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
    {
        return Err(bad());
    }
    if s.contains("..") || s.contains("//") || s.contains("@{") {
        return Err(bad());
    }
    for part in s.split('/') {
        if part.is_empty()
            || part.starts_with('.')
            || part.starts_with('-')
            || part.ends_with(".lock")
        {
            return Err(bad());
        }
    }
    Ok(s)
}

/// Export/import password: 12–1024 characters. Never echoed back.
pub fn password(s: &str) -> IpcResult<&str> {
    let n = s.chars().count();
    if n < MIN_PASSWORD_CHARS {
        return Err(IpcError::bad_input(format!(
            "Use a password of at least {MIN_PASSWORD_CHARS} characters."
        )));
    }
    if n > MAX_PASSWORD_CHARS {
        return Err(IpcError::bad_input("That password is too long."));
    }
    Ok(s)
}

/// Import conflict resolutions: valid ids, `rename` needs a valid `new_id`.
pub fn import_resolutions(list: &[ImportResolution]) -> IpcResult<()> {
    for r in list {
        profile_id(&r.profile_id)?;
        match (r.action, r.new_id.as_deref()) {
            (ImportAction::Rename, Some(new_id)) => {
                profile_id(new_id)?;
            }
            (ImportAction::Rename, None) => {
                return Err(IpcError::bad_input(format!(
                    "Choose a new id for \"{}\".",
                    r.profile_id
                )))
            }
            (_, Some(_)) => {
                return Err(IpcError::bad_input(
                    "A new id is only used when renaming a profile.",
                ))
            }
            (_, None) => {}
        }
    }
    Ok(())
}

/// Repo list for `pin_all`: each one an absolute existing folder, bounded count.
pub fn repo_list(repos: &[String]) -> IpcResult<Vec<PathBuf>> {
    if repos.len() > MAX_PIN_ALL {
        return Err(IpcError::bad_input("Too many repos in one go."));
    }
    repos.iter().map(|r| existing_dir(r)).collect()
}

/// Field-level checks the core validator doesn't make (lengths, control
/// characters that could break a git config value). Returned as validation
/// issues so the form can show them inline. Empty = fine.
pub fn profile_shape(p: &Profile) -> Vec<ValidationIssue> {
    let mut out = Vec::new();
    let mut push = |field: &str, message: String| {
        out.push(ValidationIssue {
            profile_id: Some(p.id.trim().to_string()),
            field: field.to_string(),
            severity: Severity::Error,
            message,
        });
    };
    for (field, value, what) in [
        ("label", p.label.as_str(), "Label"),
        ("name", p.name.as_str(), "Name"),
        ("email", p.email.as_str(), "Email"),
    ] {
        if value.chars().any(char::is_control) {
            push(field, format!("{what} can't contain line breaks or tabs."));
        } else if value.len() > MAX_TEXT_LEN {
            push(field, format!("{what} is too long."));
        }
    }
    let key = p.ssh_key.to_string_lossy();
    if key.chars().any(char::is_control) {
        push(
            "ssh_key",
            "The key path contains invalid characters.".to_string(),
        );
    } else if key.len() > MAX_PATH_LEN {
        push("ssh_key", "The key path is too long.".to_string());
    } else if !key.trim().is_empty()
        && Path::new(key.as_ref())
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        push(
            "ssh_key",
            "Key paths with \"..\" aren't allowed.".to_string(),
        );
    }
    if p.owners.len() > MAX_LIST_LEN {
        push("owners", "That's too many owners.".to_string());
    } else if p
        .owners
        .iter()
        .any(|o| o.len() > MAX_TEXT_LEN || o.chars().any(char::is_control))
    {
        push("owners", "An owner name isn't valid.".to_string());
    }
    if p.folders.len() > MAX_LIST_LEN {
        push("folders", "That's too many folders.".to_string());
    } else if p
        .folders
        .iter()
        .any(|f| f.len() > MAX_PATH_LEN || f.chars().any(char::is_control))
    {
        push("folders", "A folder path isn't valid.".to_string());
    }
    out
}

/// Text safe to echo in a message: control characters dropped, length capped.
fn printable(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).take(80).collect();
    if s.chars().count() > 80 {
        format!("{clean}…")
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::ErrorCode;
    use githop_core::ProfileColor;

    fn profile(id: &str) -> Profile {
        Profile {
            id: id.into(),
            label: "Office".into(),
            name: "Harshal".into(),
            email: "h@company.com".into(),
            color: ProfileColor::Blue,
            ssh_key: PathBuf::from("C:/Users/h/.ssh/githop_office"),
            owners: vec!["office-org".into()],
            folders: vec!["C:/Office/".into()],
        }
    }

    #[test]
    fn ids() {
        for ok in ["office", "a", "my-work-2", &"a".repeat(32)] {
            assert!(profile_id(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Office",
            "of fice",
            "office/../x",
            "../etc",
            "office\n",
            "ö",
            "a_b",
            "x;rm",
            &"a".repeat(33),
        ] {
            let e = profile_id(bad).unwrap_err();
            assert_eq!(e.code, ErrorCode::BadInput, "{bad:?}");
        }
    }

    #[test]
    fn relative_and_traversal_paths_are_refused() {
        for bad in [
            "",
            "   ",
            "relative/dir",
            "./x",
            "..",
            "C:relative",
            "C:/Office/../Windows",
            r"C:\Office\..\Windows",
            r"\\?\C:\x",
            r"\\.\pipe\x",
            "C:/a\u{0}b",
            "C:/a\nb",
        ] {
            let e = abs_path(bad).unwrap_err();
            assert_eq!(e.code, ErrorCode::BadInput, "{bad:?}");
        }
        assert!(abs_path(&format!("C:/{}", "a".repeat(5000))).is_err());
    }

    #[test]
    fn existing_dir_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().to_string();
        assert!(existing_dir(&dir).is_ok());
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        let file_s = file.to_string_lossy().to_string();
        assert!(existing_file(&file_s).is_ok());
        assert_eq!(existing_dir(&file_s).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(existing_file(&dir).unwrap_err().code, ErrorCode::NotFound);
        let missing = tmp.path().join("nope").to_string_lossy().to_string();
        assert_eq!(
            existing_dir(&missing).unwrap_err().code,
            ErrorCode::NotFound
        );
        // New file: parent must exist; must not be a folder.
        let new = tmp
            .path()
            .join("export.githop")
            .to_string_lossy()
            .to_string();
        assert!(new_file_in_existing_dir(&new).is_ok());
        let orphan = tmp
            .path()
            .join("nope")
            .join("x.githop")
            .to_string_lossy()
            .to_string();
        assert_eq!(
            new_file_in_existing_dir(&orphan).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            new_file_in_existing_dir(&dir).unwrap_err().code,
            ErrorCode::BadInput
        );
        assert!(new_file_in_existing_dir("export.githop").is_err());
    }

    #[test]
    fn stages() {
        assert_eq!(stage("commit").unwrap(), "commit");
        assert_eq!(stage("push").unwrap(), "push");
        assert_eq!(stage("clone").unwrap(), "clone");
        for bad in ["", "Commit", "pre-commit", "merge", "commit; rm -rf /"] {
            assert_eq!(stage(bad).unwrap_err().code, ErrorCode::BadInput, "{bad}");
        }
    }

    #[test]
    fn refs() {
        for ok in [
            "refs/githop/backup/main/20261031T142233.123Z",
            "refs/githop/backup/feature/x_y/20261031T142233.123Z-01",
            "refs/githop/backup/fix+ü/20261031T142233.123Z",
        ] {
            assert!(backup_ref(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "main",
            "refs/heads/main",
            "refs/githop/backup/",
            "refs/githop/backup/../x",
            "refs/githop/backup//x",
            "refs/githop/backup/-x",
            "refs/githop/backup/.x",
            "refs/githop/backup/x.lock",
            "refs/githop/backup/x.",
            "refs/githop/backup/a b",
            "refs/githop/backup/a@{1}",
            "refs/githop/backup/a:b",
            "refs/githop/backup/a\nb",
            "--upload-pack=evil",
        ] {
            assert!(backup_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn passwords() {
        assert!(password("short").is_err());
        assert!(password("elevenchars").is_err());
        assert!(password("twelve chars").is_ok());
        assert!(password(&"x".repeat(2000)).is_err());
        // Counted in characters, not bytes.
        assert!(password("ééééééééééé").is_err());
    }

    #[test]
    fn resolutions() {
        let ok = vec![
            ImportResolution {
                profile_id: "office".into(),
                action: ImportAction::Skip,
                new_id: None,
            },
            ImportResolution {
                profile_id: "personal".into(),
                action: ImportAction::Rename,
                new_id: Some("personal-2".into()),
            },
        ];
        assert!(import_resolutions(&ok).is_ok());
        let missing_new = vec![ImportResolution {
            profile_id: "a".into(),
            action: ImportAction::Rename,
            new_id: None,
        }];
        assert!(import_resolutions(&missing_new).is_err());
        let bad_new = vec![ImportResolution {
            profile_id: "a".into(),
            action: ImportAction::Rename,
            new_id: Some("../x".into()),
        }];
        assert!(import_resolutions(&bad_new).is_err());
        let stray_new = vec![ImportResolution {
            profile_id: "a".into(),
            action: ImportAction::Replace,
            new_id: Some("b".into()),
        }];
        assert!(import_resolutions(&stray_new).is_err());
        let bad_id = vec![ImportResolution {
            profile_id: "A".into(),
            action: ImportAction::Skip,
            new_id: None,
        }];
        assert!(import_resolutions(&bad_id).is_err());
    }

    #[test]
    fn repo_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().to_string();
        assert_eq!(repo_list(std::slice::from_ref(&dir)).unwrap().len(), 1);
        assert!(repo_list(&[dir, "relative".into()]).is_err());
        assert!(repo_list(&vec!["C:/x".to_string(); MAX_PIN_ALL + 1]).is_err());
    }

    #[test]
    fn profile_shape_catches_control_chars_and_lengths() {
        assert!(profile_shape(&profile("office")).is_empty());

        let mut p = profile("office");
        p.name = "Harshal\n[core]\n\thooksPath = /tmp".into();
        let issues = profile_shape(&p);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].field, "name");
        assert_eq!(issues[0].severity, Severity::Error);

        let mut p = profile("office");
        p.email = "a".repeat(300);
        assert_eq!(profile_shape(&p)[0].field, "email");

        let mut p = profile("office");
        p.ssh_key = PathBuf::from("C:/Users/h/../../Windows/key");
        assert_eq!(profile_shape(&p)[0].field, "ssh_key");

        let mut p = profile("office");
        p.owners = vec!["x".into(); MAX_LIST_LEN + 1];
        assert_eq!(profile_shape(&p)[0].field, "owners");

        let mut p = profile("office");
        p.folders = vec!["C:/a\u{7}".into()];
        assert_eq!(profile_shape(&p)[0].field, "folders");
    }

    #[test]
    fn messages_never_echo_control_chars() {
        let e = profile_id("bad\u{1b}[31mid").unwrap_err();
        assert!(!e.message.chars().any(char::is_control));
    }
}
