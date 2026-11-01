//! Profile model and validation (Plan §6, §8 C5/C6; Design §2.2).
//!
//! CONTRACT FILE: type definitions and signatures are fixed by the architect.
//! Implementations marked `todo!()` are filled in by the Phase 1 "model & storage" task.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths::{folder_key, is_absolute_str, normalize_folder};

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
    ///
    /// Also: empty owners/folders are dropped, duplicates within the profile are
    /// removed (first one kept), and `\` in `ssh_key` becomes `/`.
    pub fn normalize(&mut self) {
        trim_in_place(&mut self.id);
        trim_in_place(&mut self.label);
        trim_in_place(&mut self.name);
        trim_in_place(&mut self.email);

        if let Some(s) = self.ssh_key.to_str() {
            let cleaned = s.trim().replace('\\', "/");
            if cleaned != s {
                self.ssh_key = PathBuf::from(cleaned);
            }
        }

        let mut owners: Vec<String> = Vec::with_capacity(self.owners.len());
        for owner in self.owners.drain(..) {
            let o = owner.trim().to_lowercase();
            if !o.is_empty() && !owners.contains(&o) {
                owners.push(o);
            }
        }
        self.owners = owners;

        let mut folders: Vec<String> = Vec::with_capacity(self.folders.len());
        let mut keys: Vec<String> = Vec::with_capacity(self.folders.len());
        for folder in self.folders.drain(..) {
            let trimmed = folder.trim();
            if trimmed.is_empty() {
                continue;
            }
            let f = normalize_folder(trimmed).unwrap_or_else(|| trimmed.to_string());
            let key = folder_key(&f);
            if !keys.contains(&key) {
                keys.push(key);
                folders.push(f);
            }
        }
        self.folders = folders;
    }

    /// Name used in messages: the label, or the id when the label is empty.
    fn display_name(&self) -> &str {
        let label = self.label.trim();
        if label.is_empty() {
            self.id.trim()
        } else {
            label
        }
    }

    /// Looks like a personal identity (label or id mentions "personal"/"home").
    fn looks_personal(&self) -> bool {
        [self.id.to_lowercase(), self.label.to_lowercase()]
            .iter()
            .any(|s| s.contains("personal") || s.contains("home"))
    }
}

fn trim_in_place(s: &mut String) {
    let t = s.trim();
    if t.len() != s.len() {
        *s = t.to_string();
    }
}

/// Max length of a profile id.
const ID_MAX_LEN: usize = 32;

/// GitHub's noreply email domain (Plan §6).
const NOREPLY_SUFFIX: &str = "@users.noreply.github.com";

/// `^[a-z0-9-]{1,32}$` without a regex crate.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_MAX_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Basic email syntax: `local@domain.tld`, one `@`, no spaces or `<>,;"`.
fn is_valid_email(email: &str) -> bool {
    if email
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"'))
    {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && !domain.starts_with('-')
}

/// GitHub user/org name: letters, numbers, `-`, `_`, `.` (no slashes, spaces or URLs).
fn is_valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Small builder so each check reads as one line.
struct Issues(Vec<ValidationIssue>);

impl Issues {
    fn push(
        &mut self,
        profile: Option<&Profile>,
        field: &str,
        severity: Severity,
        message: String,
    ) {
        self.0.push(ValidationIssue {
            profile_id: profile.map(|p| p.id.trim().to_string()),
            field: field.to_string(),
            severity,
            message,
        });
    }

    fn error(&mut self, p: &Profile, field: &str, message: String) {
        self.push(Some(p), field, Severity::Error, message);
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
        for p in &mut self.profiles {
            p.normalize();
        }
        if let Some(d) = &mut self.default {
            trim_in_place(d);
        }
    }

    /// Every issue (errors, warnings, info) for the whole set, in one pass.
    ///
    /// Works on un-normalised input too: comparisons trim and ignore case where
    /// the rule says so. Clashes between profiles are reported on the later
    /// profile, naming the earlier one.
    ///
    /// Exactly the same folder in two profiles is an error (no rule can win);
    /// one folder inside another's is info (longest prefix wins, Plan §8 C6).
    pub fn issues(&self) -> Vec<ValidationIssue> {
        let mut out = Issues(Vec::new());

        for (i, p) in self.profiles.iter().enumerate() {
            let earlier = &self.profiles[..i];
            check_id(&mut out, p, earlier);
            check_text(&mut out, p);
            check_email(&mut out, p, earlier);
            if let Some(other) = earlier.iter().find(|o| o.color == p.color) {
                out.error(
                    p,
                    "color",
                    format!(
                        "{} already uses the {} colour. Pick another colour.",
                        quoted_profile(other),
                        p.color.as_str()
                    ),
                );
            }
            check_ssh_key(&mut out, p);
            check_shape(&mut out, p);
            check_owners(&mut out, p, earlier);
            check_folders(&mut out, p, earlier);
        }

        match self.default.as_deref().map(str::trim) {
            Some("") => out.push(
                None,
                "default",
                Severity::Error,
                "The default profile is empty. Pick one of your profiles.".to_string(),
            ),
            Some(d) if !self.profiles.iter().any(|p| p.id.trim() == d) => out.push(
                None,
                "default",
                Severity::Error,
                format!("The default profile \"{d}\" doesn't exist. Pick one of your profiles."),
            ),
            _ => {}
        }

        out.0
    }

    /// `Ok(())` when there are no `Severity::Error` issues; otherwise `Err` with **all** issues.
    pub fn validate(&self) -> Result<(), Vec<ValidationIssue>> {
        let issues = self.issues();
        if issues.iter().any(|i| i.severity == Severity::Error) {
            Err(issues)
        } else {
            Ok(())
        }
    }

    /// First colour in [`ProfileColor::ALL`] not used by any profile.
    pub fn next_free_color(&self) -> Option<ProfileColor> {
        ProfileColor::ALL
            .into_iter()
            .find(|c| !self.profiles.iter().any(|p| p.color == *c))
    }
}

/// `Profile "Office"` reference at the start of a sentence.
fn quoted_profile(p: &Profile) -> String {
    format!("Profile \"{}\"", p.display_name())
}

/// `profile "Office"` reference inside a sentence.
fn profile_ref(p: &Profile) -> String {
    format!("profile \"{}\"", p.display_name())
}

fn check_id(out: &mut Issues, p: &Profile, earlier: &[Profile]) {
    let id = p.id.trim();
    if id.is_empty() {
        out.error(p, "id", "Add a profile ID, like office.".to_string());
    } else if !is_valid_id(id) {
        out.error(
            p,
            "id",
            format!(
                "Profile ID \"{id}\" can only use lower-case letters, numbers and dashes \
                 (up to {ID_MAX_LEN} characters)."
            ),
        );
    }
    if !id.is_empty() && earlier.iter().any(|o| o.id.trim() == id) {
        out.error(
            p,
            "id",
            format!("Another profile already uses the ID \"{id}\". Each profile needs its own ID."),
        );
    }
}

fn check_text(out: &mut Issues, p: &Profile) {
    if p.label.trim().is_empty() {
        out.error(
            p,
            "label",
            "Add a label for this profile, like Office.".to_string(),
        );
    }
    let name = p.name.trim();
    if name.is_empty() {
        out.error(p, "name", "Add the name to use on commits.".to_string());
    } else if name
        .chars()
        .any(|c| matches!(c, '<' | '>') || c.is_control())
    {
        out.error(
            p,
            "name",
            "The name can't contain < or > or line breaks. git doesn't allow them.".to_string(),
        );
    }
}

fn check_email(out: &mut Issues, p: &Profile, earlier: &[Profile]) {
    let email = p.email.trim();
    if email.is_empty() {
        out.error(p, "email", "Add the email to use on commits.".to_string());
        return;
    }
    if !is_valid_email(email) {
        out.error(
            p,
            "email",
            format!("\"{email}\" doesn't look like an email. Use the form name@example.com."),
        );
    }
    if let Some(other) = earlier
        .iter()
        .find(|o| o.email.trim().to_lowercase() == email.to_lowercase())
    {
        out.error(
            p,
            "email",
            format!(
                "{} already uses this email. Each profile needs its own email.",
                quoted_profile(other)
            ),
        );
    }
    if p.looks_personal() && !email.to_lowercase().ends_with(NOREPLY_SUFFIX) {
        out.push(
            Some(p),
            "email",
            Severity::Warning,
            "This looks like a personal profile. Consider your GitHub noreply email \
             (…@users.noreply.github.com) so your real email stays private."
                .to_string(),
        );
    }
}

fn check_ssh_key(out: &mut Issues, p: &Profile) {
    let key = p.ssh_key.to_string_lossy();
    let key = key.trim();
    if key.is_empty() {
        out.error(
            p,
            "ssh_key",
            "Add the path to this profile's SSH key.".to_string(),
        );
    } else if !is_absolute_str(key) {
        out.error(
            p,
            "ssh_key",
            format!("SSH key \"{key}\" must be a full path, like C:/Users/you/.ssh/githop_office."),
        );
    }
    // ssh expands `%` tokens and `${VAR}` in identity paths (review #6). GitHop
    // escapes `%`, but there's no escape for `${`, and other tools may not escape either.
    if key.contains("${") {
        out.push(
            Some(p),
            "ssh_key",
            Severity::Warning,
            format!(
                "SSH key \"{key}\" contains ${{. ssh reads that as an environment variable, \
                 so it may not find the key. Use a path without ${{."
            ),
        );
    } else if key.contains('%') {
        out.push(
            Some(p),
            "ssh_key",
            Severity::Warning,
            format!(
                "SSH key \"{key}\" contains %. GitHop escapes it for ssh, but other tools may \
                 read it differently. Consider a path without %."
            ),
        );
    }
}

/// Longest label or commit name (characters).
pub const MAX_NAME_LEN: usize = 128;
/// Longest email (RFC 5321 path limit).
pub const MAX_EMAIL_LEN: usize = 254;
/// Longest key or folder path.
pub const MAX_PATH_LEN: usize = 4096;
/// Most owners or folders per profile.
pub const MAX_LIST_LEN: usize = 100;

/// Shape rules shared by the CLI, import, the app and hand-edited files
/// (security regression #4, parity with the app's `profile_shape`): no
/// control characters in any text field or path (they could carry terminal
/// escapes, or leave a pinned repo blocked for good), length caps, and no
/// `"` or `..` in the key path.
fn check_shape(out: &mut Issues, p: &Profile) {
    if p.label.chars().any(char::is_control) {
        out.error(
            p,
            "label",
            "The label can't contain line breaks, tabs or other control characters.".to_string(),
        );
    } else if p.label.chars().count() > MAX_NAME_LEN {
        out.error(p, "label", format!("The label is too long (up to {MAX_NAME_LEN} characters)."));
    }
    if p.name.chars().count() > MAX_NAME_LEN {
        out.error(p, "name", format!("The name is too long (up to {MAX_NAME_LEN} characters)."));
    }
    if p.email.chars().any(char::is_control) {
        out.error(
            p,
            "email",
            "The email can't contain line breaks or other control characters.".to_string(),
        );
    } else if p.email.trim().len() > MAX_EMAIL_LEN {
        out.error(p, "email", format!("The email is too long (up to {MAX_EMAIL_LEN} characters)."));
    }
    let key = p.ssh_key.to_string_lossy();
    if key.chars().any(|c| c.is_control() || c == '"') {
        out.error(
            p,
            "ssh_key",
            "The key path can't contain quotes, line breaks or other control characters."
                .to_string(),
        );
    } else if key.len() > MAX_PATH_LEN {
        out.error(p, "ssh_key", "The key path is too long.".to_string());
    } else if key
        .split(['/', '\\'])
        .any(|part| part.trim() == "..")
    {
        out.error(p, "ssh_key", "Key paths with \"..\" aren't allowed.".to_string());
    }
    if p.owners.len() > MAX_LIST_LEN {
        out.error(p, "owners", format!("That's too many owners (up to {MAX_LIST_LEN})."));
    } else if p
        .owners
        .iter()
        .any(|o| o.chars().count() > MAX_NAME_LEN || o.chars().any(char::is_control))
    {
        out.error(p, "owners", "An owner name isn't valid.".to_string());
    }
    if p.folders.len() > MAX_LIST_LEN {
        out.error(p, "folders", format!("That's too many folders (up to {MAX_LIST_LEN})."));
    } else if p
        .folders
        .iter()
        .any(|f| f.len() > MAX_PATH_LEN || f.chars().any(char::is_control))
    {
        out.error(p, "folders", "A folder path isn't valid.".to_string());
    }
}

/// `Err` with the first error message when this one profile has errors in
/// its own fields (not clashes with others): for `pin`, `use` and Fix,
/// which must not act on a hand-edited profile that doesn't validate.
pub fn profile_errors(set: &ProfileSet, id: &str) -> Option<String> {
    set.issues()
        .into_iter()
        .find(|i| i.severity == Severity::Error && i.profile_id.as_deref() == Some(id))
        .map(|i| i.message)
}

fn check_owners(out: &mut Issues, p: &Profile, earlier: &[Profile]) {
    let mut seen: Vec<String> = Vec::new();
    for raw in &p.owners {
        let owner = raw.trim().to_lowercase();
        if owner.is_empty() {
            continue;
        }
        if seen.contains(&owner) {
            continue;
        }
        seen.push(owner.clone());
        if !is_valid_owner(&owner) {
            out.error(
                p,
                "owners",
                format!(
                    "Owner \"{}\" isn't a GitHub user or organisation name. \
                     Use just the name, like office-org.",
                    raw.trim()
                ),
            );
            continue;
        }
        if let Some(other) = earlier
            .iter()
            .find(|o| o.owners.iter().any(|x| x.trim().to_lowercase() == owner))
        {
            out.error(
                p,
                "owners",
                format!(
                    "Owner \"{owner}\" already belongs to {}. Each owner can belong to one profile.",
                    profile_ref(other)
                ),
            );
        }
    }
}

fn check_folders(out: &mut Issues, p: &Profile, earlier: &[Profile]) {
    let mut seen: Vec<String> = Vec::new();
    for raw in &p.folders {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(norm) = normalize_folder(trimmed) else {
            out.error(
                p,
                "folders",
                format!("Folder \"{trimmed}\" must be a full path, like C:/Office/."),
            );
            continue;
        };
        let key = folder_key(&norm);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());

        for other in earlier {
            for of in other
                .folders
                .iter()
                .filter_map(|f| normalize_folder(f.trim()))
            {
                let okey = folder_key(&of);
                if okey == key {
                    out.error(
                        p,
                        "folders",
                        format!(
                            "Folder {norm} is already a rule for {}. A folder can belong to \
                             one profile.",
                            profile_ref(other)
                        ),
                    );
                } else if key.starts_with(&okey) || okey.starts_with(&key) {
                    let (inner, inner_p, outer, outer_p) = if key.len() > okey.len() {
                        (&norm, p, &of, other)
                    } else {
                        (&of, other, &norm, p)
                    };
                    out.push(
                        Some(p),
                        "folders",
                        Severity::Info,
                        format!(
                            "Folder {inner} ({}) is inside {outer} ({}). Repos in {inner} use \
                             {} because the longest folder rule wins.",
                            inner_p.display_name(),
                            outer_p.display_name(),
                            inner_p.display_name()
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, color: ProfileColor) -> Profile {
        Profile {
            id: id.to_string(),
            label: id.to_uppercase(),
            name: format!("{id} person"),
            email: format!("{id}@example.com"),
            color,
            ssh_key: PathBuf::from(format!("C:/Users/me/.ssh/githop_{id}")),
            owners: vec![format!("{id}-org")],
            folders: vec![format!("C:/{id}/")],
        }
    }

    fn set(profiles: Vec<Profile>) -> ProfileSet {
        ProfileSet {
            version: PROFILES_VERSION,
            default: profiles.first().map(|p| p.id.clone()),
            profiles,
        }
    }

    fn office_personal() -> ProfileSet {
        let mut personal = profile("personal", ProfileColor::Violet);
        personal.email = "12345+me@users.noreply.github.com".into();
        set(vec![profile("office", ProfileColor::Blue), personal])
    }

    fn find<'a>(
        issues: &'a [ValidationIssue],
        id: Option<&str>,
        field: &str,
        sev: Severity,
    ) -> Vec<&'a ValidationIssue> {
        issues
            .iter()
            .filter(|i| i.profile_id.as_deref() == id && i.field == field && i.severity == sev)
            .collect()
    }

    fn assert_sentence(msg: &str) {
        let first = msg.chars().next().unwrap();
        assert!(!first.is_lowercase(), "should start with a capital: {msg}");
        assert!(msg.ends_with('.'), "should end with a full stop: {msg}");
    }

    #[test]
    fn valid_set_has_no_issues() {
        let s = office_personal();
        assert_eq!(s.issues(), vec![]);
        assert!(s.validate().is_ok());
        assert!(ProfileSet::default().validate().is_ok());
    }

    #[test]
    fn id_rules() {
        let too_long = "a".repeat(33);
        let max = "a".repeat(32);
        for bad in [
            "",
            "Office",
            "off ice",
            "off_ice",
            "caf\u{e9}",
            too_long.as_str(),
        ] {
            let mut s = office_personal();
            s.profiles[0].id = bad.to_string();
            s.default = None;
            let issues = s.issues();
            assert_eq!(
                find(&issues, Some(bad), "id", Severity::Error).len(),
                1,
                "id {bad:?}: {issues:?}"
            );
            assert!(s.validate().is_err());
        }
        for good in ["a", "office-2", max.as_str(), "0-0"] {
            let mut s = office_personal();
            s.profiles[0].id = good.to_string();
            s.default = None;
            assert!(s.validate().is_ok(), "id {good:?}");
        }
    }

    #[test]
    fn duplicates_are_errors_on_later_profile() {
        let mut s = office_personal();
        s.profiles[1].id = "office".into();
        s.profiles[1].email = "OFFICE@Example.com".into();
        s.profiles[1].color = ProfileColor::Blue;
        s.profiles[1].owners = vec!["Office-Org".into()];
        s.profiles[1].folders = vec![r"c:\office".into()];
        let issues = s.validate().unwrap_err();
        for field in ["id", "email", "color", "owners", "folders"] {
            let hits = find(&issues, Some("office"), field, Severity::Error);
            assert_eq!(hits.len(), 1, "{field}: {issues:?}");
            assert_sentence(&hits[0].message);
        }
        // Every clash is reported once, on the later profile, naming the earlier one.
        let errors = issues.iter().filter(|i| i.severity == Severity::Error);
        assert_eq!(errors.count(), 5, "{issues:?}");
        let owner_msg = &find(&issues, Some("office"), "owners", Severity::Error)[0].message;
        assert!(owner_msg.contains("office-org") && owner_msg.contains("\"OFFICE\""));
    }

    #[test]
    fn returns_every_issue_at_once() {
        let s = ProfileSet {
            version: 1,
            default: Some("nobody".into()),
            profiles: vec![Profile {
                id: "Bad Id".into(),
                label: " ".into(),
                name: String::new(),
                email: "not-an-email".into(),
                color: ProfileColor::Blue,
                ssh_key: PathBuf::from(".ssh/key"),
                owners: vec!["https://github.com/x".into()],
                folders: vec!["relative/dir".into()],
            }],
        };
        let issues = s.validate().unwrap_err();
        let fields: Vec<&str> = issues.iter().map(|i| i.field.as_str()).collect();
        assert_eq!(
            fields,
            ["id", "label", "name", "email", "ssh_key", "owners", "folders", "default"]
        );
        assert!(issues.iter().all(|i| i.severity == Severity::Error));
        assert_eq!(issues.last().unwrap().profile_id, None);
        for i in &issues {
            assert_sentence(&i.message);
        }
    }

    #[test]
    fn empty_email_and_key() {
        let mut s = office_personal();
        s.profiles[0].email = "  ".into();
        s.profiles[0].ssh_key = PathBuf::new();
        let issues = s.validate().unwrap_err();
        assert_eq!(
            find(&issues, Some("office"), "email", Severity::Error).len(),
            1
        );
        assert_eq!(
            find(&issues, Some("office"), "ssh_key", Severity::Error).len(),
            1
        );
    }

    #[test]
    fn name_with_angle_brackets_is_error() {
        let mut s = office_personal();
        s.profiles[0].name = "Me <me@x.com>".into();
        let issues = s.validate().unwrap_err();
        assert_eq!(
            find(&issues, Some("office"), "name", Severity::Error).len(),
            1
        );
    }

    #[test]
    fn email_syntax() {
        for good in [
            "a@b.co",
            "first.last+tag@sub.example.org",
            "12345+me@users.noreply.github.com",
        ] {
            assert!(is_valid_email(good), "{good}");
        }
        for bad in [
            "plain",
            "@b.co",
            "a@",
            "a@b",
            "a@@b.co",
            "a@b@c.co",
            "a b@c.co",
            "a@.b.co",
            "a@b.co.",
            "a@b..co",
            "<a@b.co>",
            "a@b.co,c@d.co",
        ] {
            assert!(!is_valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn personal_without_noreply_is_warning_only() {
        let mut s = office_personal();
        s.profiles[1].email = "me@gmail.com".into();
        let issues = s.issues();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].severity, Severity::Warning);
        assert_eq!(issues[0].field, "email");
        assert_eq!(issues[0].profile_id.as_deref(), Some("personal"));
        assert_sentence(&issues[0].message);
        assert!(s.validate().is_ok());

        // "home" in the label counts too; the noreply check ignores case.
        let mut s = office_personal();
        s.profiles[0].label = "Home stuff".into();
        assert_eq!(
            find(&s.issues(), Some("office"), "email", Severity::Warning).len(),
            1
        );
        s.profiles[0].email = "1+x@USERS.NOREPLY.GITHUB.COM".into();
        assert!(s.issues().is_empty());
    }

    #[test]
    fn ssh_key_must_be_absolute_but_need_not_exist() {
        let mut s = office_personal();
        s.profiles[0].ssh_key = PathBuf::from(r"Z:\definitely\missing\key");
        assert!(s.validate().is_ok());
        s.profiles[0].ssh_key = PathBuf::from("//server/share/key");
        assert!(s.validate().is_ok());
        s.profiles[0].ssh_key = PathBuf::from("~/.ssh/key");
        let issues = s.validate().unwrap_err();
        assert_eq!(
            find(&issues, Some("office"), "ssh_key", Severity::Error).len(),
            1
        );
    }

    #[test]
    fn ssh_key_with_percent_or_env_var_warns() {
        let mut s = office_personal();
        s.profiles[0].ssh_key = PathBuf::from("C:/Users/me/100%/githop_office");
        let issues = s.issues();
        let w = find(&issues, Some("office"), "ssh_key", Severity::Warning);
        assert_eq!(w.len(), 1, "{issues:?}");
        assert!(w[0].message.contains('%'));
        assert_sentence(&w[0].message);
        assert!(s.validate().is_ok());

        s.profiles[0].ssh_key = PathBuf::from("C:/Users/${USER}/key");
        let issues = s.issues();
        let w = find(&issues, Some("office"), "ssh_key", Severity::Warning);
        assert_eq!(w.len(), 1, "{issues:?}");
        assert!(w[0].message.contains("${"));
        assert_sentence(&w[0].message);
    }

    #[test]
    fn overlapping_folders_are_info() {
        let mut s = office_personal();
        s.profiles[0].folders = vec!["C:/Work/".into()];
        s.profiles[1].folders = vec![r"C:\work\Side".into()];
        let issues = s.issues();
        let info = find(&issues, Some("personal"), "folders", Severity::Info);
        assert_eq!(info.len(), 1, "{issues:?}");
        assert!(
            info[0].message.contains("C:/work/Side/"),
            "{}",
            info[0].message
        );
        assert!(info[0].message.contains("longest folder rule wins"));
        assert_sentence(&info[0].message);
        assert!(s.validate().is_ok());

        // Same overlap with the outer folder declared later.
        s.profiles.swap(0, 1);
        let issues = s.issues();
        assert_eq!(
            find(&issues, Some("office"), "folders", Severity::Info).len(),
            1
        );

        // Siblings sharing a name prefix don't overlap.
        let mut s = office_personal();
        s.profiles[0].folders = vec!["C:/Work/".into()];
        s.profiles[1].folders = vec!["C:/Workshop/".into()];
        assert!(s.issues().is_empty());
    }

    #[test]
    fn default_must_exist() {
        let mut s = office_personal();
        s.default = Some("nope".into());
        let issues = s.validate().unwrap_err();
        assert_eq!(find(&issues, None, "default", Severity::Error).len(), 1);
        s.default = Some(String::new());
        assert_eq!(find(&s.issues(), None, "default", Severity::Error).len(), 1);
        s.default = None;
        assert!(s.validate().is_ok());
        s.default = Some("personal".into());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn normalize_profile() {
        let mut p = Profile {
            id: " office ".into(),
            label: " Office ".into(),
            name: " Me ".into(),
            email: " Me@Example.com ".into(),
            color: ProfileColor::Blue,
            ssh_key: PathBuf::from(r" C:\Users\me\.ssh\key "),
            owners: vec![" Office-Org ".into(), "office-org".into(), " ".into()],
            folders: vec![
                r"C:\My Work".into(),
                "c:/my work/".into(),
                "relative".into(),
                String::new(),
            ],
        };
        p.normalize();
        assert_eq!(p.id, "office");
        assert_eq!(p.label, "Office");
        assert_eq!(p.name, "Me");
        assert_eq!(p.email, "Me@Example.com");
        assert_eq!(p.ssh_key, PathBuf::from("C:/Users/me/.ssh/key"));
        assert_eq!(p.owners, vec!["office-org"]);
        assert_eq!(p.folders, vec!["C:/My Work/", "relative"]);

        let mut s = set(vec![p]);
        s.default = Some(" office ".into());
        s.normalize();
        assert_eq!(s.default.as_deref(), Some("office"));
        // The relative folder survives normalisation so validation can report it.
        let issues = s.validate().unwrap_err();
        assert_eq!(
            find(&issues, Some("office"), "folders", Severity::Error).len(),
            1
        );
    }

    #[test]
    fn next_free_color_in_order() {
        let mut s = ProfileSet::default();
        assert_eq!(s.next_free_color(), Some(ProfileColor::Blue));
        s.profiles.push(profile("a", ProfileColor::Blue));
        s.profiles.push(profile("b", ProfileColor::Pink));
        assert_eq!(s.next_free_color(), Some(ProfileColor::Violet));
        s.profiles.clear();
        for (i, c) in ProfileColor::ALL.into_iter().enumerate() {
            s.profiles.push(profile(&format!("p{i}"), c));
        }
        assert_eq!(s.next_free_color(), None);
    }

    #[test]
    fn color_parse_and_names() {
        for c in ProfileColor::ALL {
            assert_eq!(ProfileColor::parse(c.as_str()), Some(c));
            assert_eq!(ProfileColor::parse(&c.as_str().to_uppercase()), Some(c));
        }
        assert_eq!(ProfileColor::parse("red"), None);
    }
}
