//! Profile model and validation (Plan §6, §8 C5/C6; Design §2.2).
//!
//! CONTRACT FILE: type definitions and signatures are fixed by the architect.
//! Implementations marked `todo!()` are filled in by the Phase 1 "model & storage" task.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Current `profiles.toml` schema version.
pub const PROFILES_VERSION: u32 = 1;

/// Profile colour, stored by name (Design §2.2). Order = assignment order for new profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileColor {
    Blue,
    Violet,
    Pink,
    Indigo,
    Plum,
    Slate,
    Navy,
    Steel,
}

impl ProfileColor {
    /// All colours in assignment order.
    pub const ALL: [ProfileColor; 8] = [
        ProfileColor::Blue,
        ProfileColor::Violet,
        ProfileColor::Pink,
        ProfileColor::Indigo,
        ProfileColor::Plum,
        ProfileColor::Slate,
        ProfileColor::Navy,
        ProfileColor::Steel,
    ];

    /// Lower-case name as stored in TOML (`"blue"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileColor::Blue => "blue",
            ProfileColor::Violet => "violet",
            ProfileColor::Pink => "pink",
            ProfileColor::Indigo => "indigo",
            ProfileColor::Plum => "plum",
            ProfileColor::Slate => "slate",
            ProfileColor::Navy => "navy",
            ProfileColor::Steel => "steel",
        }
    }

    /// Parse a stored name (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

/// One git identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Slug, `^[a-z0-9-]{1,32}$`, unique.
    pub id: String,
    /// Display name, e.g. "Personal".
    pub label: String,
    /// git `user.name`.
    pub name: String,
    /// git `user.email`.
    pub email: String,
    pub color: ProfileColor,
    /// Absolute path to the private SSH key (forward slashes preferred).
    pub ssh_key: PathBuf,
    /// GitHub users/orgs owned by this profile. Lower-cased on save.
    #[serde(default)]
    pub owners: Vec<String>,
    /// Folder rules, normalised (`/`, trailing `/`).
    #[serde(default)]
    pub folders: Vec<String>,
}

/// Contents of `profiles.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSet {
    pub version: u32,
    /// Default profile id (used when a repo can't be resolved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(rename = "profile", default)]
    pub profiles: Vec<Profile>,
}

impl Default for ProfileSet {
    fn default() -> Self {
        Self {
            version: PROFILES_VERSION,
            default: None,
            profiles: Vec::new(),
        }
    }
}

/// How serious a validation issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Blocks saving.
    Error,
    /// Shown, does not block saving (e.g. personal profile without noreply email).
    Warning,
    /// Informational (e.g. overlapping folder rules — longest prefix wins).
    Info,
}

/// One validation finding, addressed to a form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// Profile the issue belongs to (`None` = set-level, e.g. bad `default`).
    pub profile_id: Option<String>,
    /// Field name: `id`, `label`, `name`, `email`, `color`, `ssh_key`, `owners`, `folders`, `default`.
    pub field: String,
    pub severity: Severity,
    /// User-facing, sentence case, follows Design §9 voice.
    pub message: String,
}

impl Profile {
    /// Lower-case owners, normalise folders (see `paths::normalize_folder`), trim text fields.
    /// Folders that can't be normalised are left as-is so validation can report them.
    pub fn normalize(&mut self) {
        todo!("Phase 1: model & storage")
    }
}

impl ProfileSet {
    /// Find a profile by id.
    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Find a profile by id (mutable).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    /// The default profile, if set and present.
    pub fn default_profile(&self) -> Option<&Profile> {
        self.default.as_deref().and_then(|id| self.get(id))
    }

    /// Normalise every profile (see [`Profile::normalize`]).
    pub fn normalize(&mut self) {
        todo!("Phase 1: model & storage")
    }

    /// Every issue (errors, warnings, info) for the whole set, in one pass.
    pub fn issues(&self) -> Vec<ValidationIssue> {
        todo!("Phase 1: model & storage")
    }

    /// `Ok(())` when there are no `Severity::Error` issues; otherwise `Err` with **all** issues.
    pub fn validate(&self) -> Result<(), Vec<ValidationIssue>> {
        todo!("Phase 1: model & storage")
    }

    /// First colour in [`ProfileColor::ALL`] not used by any profile.
    pub fn next_free_color(&self) -> Option<ProfileColor> {
        todo!("Phase 1: model & storage")
    }
}
