//! What the tray shows (Design §6.2), computed from GitHop data. Pure, so the
//! rules are unit-tested without a tray.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use githop_core::hooks::HookHealth;
use githop_core::{Paths, ProfileColor, ProfileSet};

use super::render::{profile_rgb, IconSpec, BLOCKED, TEXT_MUTED, WARNING};

/// File in the data dir holding the unix time (seconds, ASCII) of the last
/// blocked commit/push. Written by core `check` (see INTEGRATE note in
/// `tray/mod.rs`); the tray only reads it.
pub const LAST_BLOCK_FILE: &str = "last-block";

/// "Blocked recently" lasts this long (Design §6.2: last 10 min).
pub const BLOCKED_WINDOW_SECS: u64 = 10 * 60;

/// Windows tray tooltips hold at most 127 characters.
const TOOLTIP_MAX: usize = 127;

/// Overall tray state, before the "blocked recently" overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayKind {
    /// Default profile set, hooks healthy.
    Normal,
    /// No profiles / no default profile yet.
    Setup,
    /// Something stops GitHop protecting commits (hooks missing, data unreadable…).
    Error(String),
}

/// Inputs for the icon, tooltip and menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayModel {
    pub kind: TrayKind,
    /// Default profile's label, email, colour and initial (when there is one).
    pub label: Option<String>,
    pub email: Option<String>,
    pub color: Option<ProfileColor>,
    pub initial: char,
    pub has_profiles: bool,
}

/// Build the model. `profiles = Err` → data couldn't be read; `hooks = None` →
/// git isn't available.
pub fn model(profiles: Result<&ProfileSet, String>, hooks: Option<&HookHealth>) -> TrayModel {
    let set = match profiles {
        Ok(s) => s,
        Err(e) => {
            return TrayModel {
                kind: TrayKind::Error(format!("Couldn't read GitHop data: {e}")),
                label: None,
                email: None,
                color: None,
                initial: 'G',
                has_profiles: false,
            }
        }
    };
    let default = set.default_profile();
    let mut m = TrayModel {
        kind: TrayKind::Normal,
        label: default.map(|p| p.label.clone()),
        email: default.map(|p| p.email.clone()),
        color: default.map(|p| p.color),
        initial: default.map_or('G', |p| initial(&p.label, &p.id)),
        has_profiles: !set.profiles.is_empty(),
    };
    if default.is_none() {
        m.kind = TrayKind::Setup;
        return m;
    }
    m.kind = match hooks {
        None => TrayKind::Error("Git was not found. Install Git for Windows.".to_string()),
        Some(h) if !h.installed || !h.points_to_us => {
            TrayKind::Error("Protection is off. Git hooks aren't installed.".to_string())
        }
        Some(h) if !h.missing.is_empty() || h.exe_missing => {
            TrayKind::Error("Protection is off. Git hooks need repair.".to_string())
        }
        Some(h) if h.sh_path.is_none() => {
            TrayKind::Error("Git for Windows' sh wasn't found, so hooks can't run.".to_string())
        }
        Some(h) if h.config_error.is_some() => {
            TrayKind::Error("Couldn't read your global git config.".to_string())
        }
        Some(_) => TrayKind::Normal,
    };
    m
}

/// First ASCII letter/digit of the label, else of the id, else "G".
pub fn initial(label: &str, id: &str) -> char {
    label
        .chars()
        .chain(id.chars())
        .find(char::is_ascii_alphanumeric)
        .map_or('G', |c| c.to_ascii_uppercase())
}

/// Icon for the model (`blocked` only shows in the normal state). Normal =
/// profile-colour circle; setup needed and error = brand logo tile (the error
/// one with the sunshine dot), per brand/README.md "the tray icon is not the logo".
pub fn icon_spec(m: &TrayModel, blocked: bool) -> IconSpec {
    match (&m.kind, m.color) {
        (TrayKind::Normal, Some(c)) => IconSpec {
            fill: profile_rgb(c),
            filled: true,
            initial: m.initial,
            dot: blocked.then_some(BLOCKED),
            logo: false,
        },
        (TrayKind::Error(_), _) => IconSpec {
            fill: TEXT_MUTED,
            filled: true,
            initial: m.initial,
            dot: Some(WARNING),
            logo: true,
        },
        _ => IconSpec {
            fill: TEXT_MUTED,
            filled: false,
            initial: 'G',
            dot: None,
            logo: true,
        },
    }
}

/// Tooltip, e.g. `GitHop — Personal (12345+harshal@users.noreply.github.com)`.
pub fn tooltip(m: &TrayModel, blocked: bool) -> String {
    let text = match &m.kind {
        TrayKind::Normal => {
            let who = match (&m.label, &m.email) {
                (Some(l), Some(e)) => format!("{l} ({e})"),
                (Some(l), None) => l.clone(),
                _ => "No default profile".to_string(),
            };
            if blocked {
                format!("GitHop — {who}\nBlocked a commit or push in the last 10 minutes.")
            } else {
                format!("GitHop — {who}")
            }
        }
        TrayKind::Setup if m.has_profiles => {
            "GitHop — No default profile. Click to choose one.".to_string()
        }
        TrayKind::Setup => "GitHop — Setup needed. Click to add your first profile.".to_string(),
        TrayKind::Error(detail) => format!("GitHop — {detail}"),
    };
    truncate(&text, TOOLTIP_MAX)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Unix time of the last block: file contents, else its modified time.
pub fn last_block(paths: &Paths) -> Option<u64> {
    read_last_block(&paths.root().join(LAST_BLOCK_FILE))
}

fn read_last_block(file: &Path) -> Option<u64> {
    if let Ok(text) = std::fs::read_to_string(file) {
        if let Ok(secs) = text.trim().parse::<u64>() {
            return Some(secs);
        }
    }
    let modified = std::fs::metadata(file).ok()?.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Seconds since the unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Was there a block within [`BLOCKED_WINDOW_SECS`] of `now`? (Future
/// timestamps — clock changes — count as recent.)
pub fn recently_blocked(last: Option<u64>, now: u64) -> bool {
    last.is_some_and(|t| now.saturating_sub(t) < BLOCKED_WINDOW_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use githop_core::Profile;
    use std::path::PathBuf;

    fn set(default: Option<&str>) -> ProfileSet {
        ProfileSet {
            version: 1,
            default: default.map(String::from),
            profiles: vec![Profile {
                id: "personal".into(),
                label: "Personal".into(),
                name: "H".into(),
                email: "1+h@users.noreply.github.com".into(),
                color: ProfileColor::Violet,
                ssh_key: PathBuf::from("C:/k"),
                owners: vec![],
                folders: vec![],
            }],
        }
    }

    fn healthy() -> HookHealth {
        HookHealth {
            installed: true,
            points_to_us: true,
            sh_path: Some(PathBuf::from("C:/Git/bin/sh.exe")),
            ..HookHealth::default()
        }
    }

    #[test]
    fn normal_state_uses_default_profile() {
        let s = set(Some("personal"));
        let m = model(Ok(&s), Some(&healthy()));
        assert_eq!(m.kind, TrayKind::Normal);
        let spec = icon_spec(&m, false);
        assert_eq!(spec.fill, profile_rgb(ProfileColor::Violet));
        assert!(spec.filled);
        assert!(
            !spec.logo,
            "normal state = profile-colour circle, not the logo"
        );
        assert_eq!(spec.initial, 'P');
        assert_eq!(spec.dot, None);
        assert_eq!(
            tooltip(&m, false),
            "GitHop — Personal (1+h@users.noreply.github.com)"
        );
        assert_eq!(icon_spec(&m, true).dot, Some(BLOCKED));
        assert!(tooltip(&m, true).contains("Blocked"));
    }

    #[test]
    fn setup_state() {
        let empty = ProfileSet::default();
        let m = model(Ok(&empty), Some(&healthy()));
        assert_eq!(m.kind, TrayKind::Setup);
        let spec = icon_spec(&m, true);
        assert!(!spec.filled);
        assert!(spec.logo, "setup needed shows the brand logo");
        assert_eq!(spec.fill, TEXT_MUTED);
        assert_eq!(spec.dot, None, "blocked dot only in normal state");
        assert!(tooltip(&m, false).contains("first profile"));
        // Profiles but no default.
        let s = set(None);
        let m = model(Ok(&s), Some(&healthy()));
        assert_eq!(m.kind, TrayKind::Setup);
        assert!(tooltip(&m, false).contains("No default profile"));
    }

    #[test]
    fn error_states() {
        let s = set(Some("personal"));
        let mut h = healthy();
        h.installed = false;
        let m = model(Ok(&s), Some(&h));
        assert!(matches!(m.kind, TrayKind::Error(_)));
        let spec = icon_spec(&m, false);
        assert_eq!(spec.dot, Some(WARNING));
        assert!(spec.logo, "error shows the brand logo + sunshine dot");
        assert_eq!(spec.fill, TEXT_MUTED);
        assert!(tooltip(&m, false).contains("Protection is off"));

        let mut h = healthy();
        h.missing = vec!["pre-commit".into()];
        assert!(matches!(model(Ok(&s), Some(&h)).kind, TrayKind::Error(_)));

        assert!(matches!(model(Ok(&s), None).kind, TrayKind::Error(_)));
        let m = model(Err("bad toml".into()), None);
        assert!(matches!(m.kind, TrayKind::Error(_)));
        assert!(tooltip(&m, false).contains("Couldn't read"));
    }

    #[test]
    fn initials() {
        assert_eq!(initial("Office", "office"), 'O');
        assert_eq!(initial("  personal", "p"), 'P');
        assert_eq!(initial("Équipe", "team"), 'Q'); // first ASCII letter of the label
        assert_eq!(initial("—", "work"), 'W');
        assert_eq!(initial("", ""), 'G');
    }

    #[test]
    fn blocked_window() {
        let now = 1_000_000;
        assert!(recently_blocked(Some(now - 60), now));
        assert!(!recently_blocked(Some(now - BLOCKED_WINDOW_SECS), now));
        assert!(recently_blocked(Some(now + 30), now));
        assert!(!recently_blocked(None, now));
    }

    #[test]
    fn last_block_file_contents_or_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path());
        assert_eq!(last_block(&paths), None);
        std::fs::write(tmp.path().join(LAST_BLOCK_FILE), "1234\n").unwrap();
        assert_eq!(last_block(&paths), Some(1234));
        std::fs::write(tmp.path().join(LAST_BLOCK_FILE), "").unwrap();
        let t = last_block(&paths).unwrap();
        assert!(now_secs().abs_diff(t) < 60);
    }

    #[test]
    fn tooltips_fit_windows_limit() {
        let mut s = set(Some("personal"));
        s.profiles[0].label = "L".repeat(200);
        let m = model(Ok(&s), Some(&healthy()));
        assert!(tooltip(&m, true).chars().count() <= TOOLTIP_MAX);
    }
}
