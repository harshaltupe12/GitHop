//! Git config snapshot (one spawn) and local/global writes (Plan §11, §8 C18).
//!
//! CONTRACT FILE: types and accessor bodies are written by the architect.
//! `ConfigSnapshot::parse`, `read` and the write functions are filled by the
//! Phase 1 "git integration" task.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::git::Git;

/// `git config` exit code for "key not found" on `--unset`/`--unset-all`.
const EXIT_KEY_MISSING: i32 = 5;

/// Where a config value came from (`--show-scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigScope {
    System,
    Global,
    Local,
    Worktree,
    Command,
    Unknown,
}

impl ConfigScope {
    pub fn parse(s: &str) -> Self {
        match s {
            "system" => ConfigScope::System,
            "global" => ConfigScope::Global,
            "local" => ConfigScope::Local,
            "worktree" => ConfigScope::Worktree,
            "command" => ConfigScope::Command,
            _ => ConfigScope::Unknown,
        }
    }
}

/// One `key=value` line from `git config --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub scope: ConfigScope,
    /// Origin as printed by `--show-origin` (e.g. `file:C:/Users/me/.gitconfig`).
    pub origin: String,
    /// Full key as git prints it: section and variable lower-case, subsection as-is
    /// (e.g. `remote.Origin.url`, `user.email`).
    pub key: String,
    /// `None` for a key with no `=` (boolean true shorthand).
    pub value: Option<String>,
}

/// All config entries visible from a repo, in git's precedence order (later wins).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSnapshot {
    entries: Vec<ConfigEntry>,
}

/// Compare keys: section + variable case-insensitive, subsection case-sensitive.
fn key_eq(a: &str, b: &str) -> bool {
    let (Some((a_sec, a_rest)), Some((b_sec, b_rest))) = (a.split_once('.'), b.split_once('.'))
    else {
        return a.eq_ignore_ascii_case(b);
    };
    if !a_sec.eq_ignore_ascii_case(b_sec) {
        return false;
    }
    match (a_rest.rsplit_once('.'), b_rest.rsplit_once('.')) {
        (Some((a_sub, a_var)), Some((b_sub, b_var))) => {
            a_sub == b_sub && a_var.eq_ignore_ascii_case(b_var)
        }
        (None, None) => a_rest.eq_ignore_ascii_case(b_rest),
        _ => false,
    }
}

impl ConfigSnapshot {
    /// Build from entries (tests, resolver unit tests).
    pub fn from_entries(entries: Vec<ConfigEntry>) -> Self {
        Self { entries }
    }

    /// Parse the NUL-separated output of
    /// `git config --list --show-origin --show-scope -z`.
    ///
    /// Each entry is three NUL-terminated fields:
    /// `<scope>\0<origin>\0<key>\n<value>\0`, or `<scope>\0<origin>\0<key>\0` for a
    /// key with no `=` (value-less boolean). Values may contain newlines; keys never
    /// do, so the first `\n` of the third field separates key from value.
    ///
    /// Never panics. Invalid UTF-8 is replaced lossily; a trailing incomplete
    /// entry (truncated output) is dropped.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let mut fields = raw.split(|b| *b == 0);
        let mut entries = Vec::new();
        while let (Some(scope), Some(origin), Some(kv)) =
            (fields.next(), fields.next(), fields.next())
        {
            let (key, value) = match kv.iter().position(|b| *b == b'\n') {
                Some(i) => (
                    &kv[..i],
                    Some(String::from_utf8_lossy(&kv[i + 1..]).into_owned()),
                ),
                None => (kv, None),
            };
            if key.is_empty() {
                continue;
            }
            entries.push(ConfigEntry {
                scope: ConfigScope::parse(&String::from_utf8_lossy(scope)),
                origin: String::from_utf8_lossy(origin).into_owned(),
                key: String::from_utf8_lossy(key).into_owned(),
                value,
            });
        }
        Ok(Self { entries })
    }

    /// Run `git config --list --show-origin --show-scope -z` in `cwd` (one spawn) and parse it.
    ///
    /// Works inside and outside a repo (outside, only system/global/command
    /// entries are listed). No config at all is an empty snapshot.
    pub fn read(git: &Git, cwd: &Path) -> Result<Self> {
        let out = git.run(
            cwd,
            &["config", "--list", "--show-origin", "--show-scope", "-z"],
        )?;
        Self::parse(&out)
    }

    pub fn entries(&self) -> &[ConfigEntry] {
        &self.entries
    }

    /// Effective (last) value of `key`. A value-less entry yields `Some("true")`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| key_eq(&e.key, key))
            .map(|e| e.value.as_deref().unwrap_or("true"))
    }

    /// Last value of `key` set at exactly `scope`.
    pub fn get_scoped(&self, key: &str, scope: ConfigScope) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.scope == scope && key_eq(&e.key, key))
            .map(|e| e.value.as_deref().unwrap_or("true"))
    }

    /// Effective entry (with scope/origin) for `key`.
    pub fn get_entry(&self, key: &str) -> Option<&ConfigEntry> {
        self.entries.iter().rev().find(|e| key_eq(&e.key, key))
    }

    /// All values of a multi-valued key, in order.
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| key_eq(&e.key, key))
            .map(|e| e.value.as_deref().unwrap_or("true"))
            .collect()
    }

    /// Remote names that have a `remote.<name>.url`, sorted, de-duplicated.
    pub fn remotes(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .entries
            .iter()
            .filter_map(|e| {
                let (section, rest) = e.key.split_once('.')?;
                let (name, var) = rest.rsplit_once('.')?;
                (section.eq_ignore_ascii_case("remote") && var.eq_ignore_ascii_case("url"))
                    .then(|| name.to_string())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// URL of remote `name` (no spawn).
    pub fn remote_url(&self, name: &str) -> Option<&str> {
        self.get(&format!("remote.{name}.url"))
    }
}

/// `git config --local <key> <value>` in `repo_root`.
///
/// Uses `--replace-all` so a key the user accidentally set twice is collapsed to
/// one value instead of failing with "multiple values" (exit 5).
pub fn set_local(git: &Git, repo_root: &Path, key: &str, value: &str) -> Result<()> {
    git.run(
        repo_root,
        &["config", "--local", "--replace-all", key, value],
    )
    .map(drop)
}

/// `git config --local --unset-all <key>`; a missing key (exit 5) is not an error.
pub fn unset_local(git: &Git, repo_root: &Path, key: &str) -> Result<()> {
    unset(git, repo_root, "--local", key)
}

/// `git config --global <key> <value>`. Caller must back up first (`backup` module).
pub fn set_global(git: &Git, key: &str, value: &str) -> Result<()> {
    git.run(
        &global_cwd(git),
        &["config", "--global", "--replace-all", key, value],
    )
    .map(drop)
}

/// `git config --global --unset-all <key>`; a missing key is not an error.
pub fn unset_global(git: &Git, key: &str) -> Result<()> {
    unset(git, &global_cwd(git), "--global", key)
}

/// Read one global value (`git config --global --get <key>`); `None` when unset.
///
/// With several values the last one wins (git's own rule).
pub fn get_global(git: &Git, key: &str) -> Result<Option<String>> {
    let args = ["config", "--global", "--get", key];
    let out = git.output(&global_cwd(git), &args, None)?;
    if out.status.success() {
        let mut v = out.stdout;
        if v.last() == Some(&b'\n') {
            v.pop();
        }
        return Ok(Some(String::from_utf8_lossy(&v).into_owned()));
    }
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    Err(git_failed(&args, &out))
}

/// `git config <scope> --unset-all <key>`, treating exit 5 (missing key) as success.
fn unset(git: &Git, cwd: &Path, scope: &str, key: &str) -> Result<()> {
    let args = ["config", scope, "--unset-all", key];
    let out = git.output(cwd, &args, None)?;
    if out.status.success() || out.status.code() == Some(EXIT_KEY_MISSING) {
        Ok(())
    } else {
        Err(git_failed(&args, &out))
    }
}

pub(crate) fn git_failed(args: &[&str], out: &std::process::Output) -> CoreError {
    CoreError::GitFailed {
        args: args.join(" "),
        code: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    }
}

/// Working dir for `--global` commands: the home dir git would use, so the
/// command never depends on (or trips over) whatever repo the caller is in.
fn global_cwd(git: &Git) -> PathBuf {
    git.home_dir()
        .filter(|h| h.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(scope: ConfigScope, key: &str, value: &str) -> ConfigEntry {
        ConfigEntry {
            scope,
            origin: String::new(),
            key: key.into(),
            value: Some(value.into()),
        }
    }

    #[test]
    fn accessors() {
        let s = ConfigSnapshot::from_entries(vec![
            e(ConfigScope::Global, "user.email", "g@x"),
            e(ConfigScope::Local, "user.email", "l@x"),
            e(ConfigScope::Local, "remote.upstream.url", "u"),
            e(ConfigScope::Local, "remote.origin.url", "o"),
        ]);
        assert_eq!(s.get("user.email"), Some("l@x"));
        assert_eq!(s.get("User.Email"), Some("l@x"));
        assert_eq!(s.get_scoped("user.email", ConfigScope::Global), Some("g@x"));
        assert_eq!(
            s.remotes(),
            vec!["origin".to_string(), "upstream".to_string()]
        );
        assert_eq!(s.remote_url("origin"), Some("o"));
        assert_eq!(s.remote_url("Origin"), None);
    }

    /// Byte layout observed from Git for Windows 2.51 (see integration test
    /// `snapshot_raw_byte_format`).
    const FIXTURE: &[u8] = b"global\0file:C:/Users/me/.gitconfig\0user.name\nJ\xc3\xb6rg \xc3\x9cnicode \xe6\x97\xa5\xe6\x9c\xac\0\
global\0file:C:/Users/me/.gitconfig\0user.email\njorg@example.com\0\
global\0file:C:/Users/me/.gitconfig\0core.flag\0\
global\0file:C:/Users/me/.gitconfig\0alias.x\n!echo a=b\nline2\0\
global\0file:C:/Users/me/work.inc\0user.email\nwork@corp.example\0\
global\0file:C:/Users/me/My Folder/inc.cfg\0foo.empty\n\0\
local\0file:.git/config\0remote.Origin.url\ngit@github.com:Owner/Repo.git\0\
worktree\0file:.git/config.worktree\0core.sparsecheckout\nfalse\0\
command\0command line:\0user.email\ncli@example.com\0";

    #[test]
    fn parse_fixture() {
        let s = ConfigSnapshot::parse(FIXTURE).unwrap();
        let en = s.entries();
        assert_eq!(en.len(), 9);
        assert_eq!(en[0].scope, ConfigScope::Global);
        assert_eq!(en[0].origin, "file:C:/Users/me/.gitconfig");
        assert_eq!(en[0].value.as_deref(), Some("Jörg Ünicode 日本"));
        // value-less key
        assert_eq!(en[2].key, "core.flag");
        assert_eq!(en[2].value, None);
        assert_eq!(s.get("core.flag"), Some("true"));
        // `=` and newline inside a value
        assert_eq!(s.get("alias.x"), Some("!echo a=b\nline2"));
        // include origin
        assert_eq!(en[4].origin, "file:C:/Users/me/work.inc");
        assert_eq!(en[5].origin, "file:C:/Users/me/My Folder/inc.cfg");
        // empty value is Some(""), not value-less
        assert_eq!(en[5].value.as_deref(), Some(""));
        assert_eq!(
            s.remote_url("Origin"),
            Some("git@github.com:Owner/Repo.git")
        );
        assert_eq!(en[7].scope, ConfigScope::Worktree);
        assert_eq!(en[8].scope, ConfigScope::Command);
        assert_eq!(en[8].origin, "command line:");
        // command line wins over global and include
        assert_eq!(s.get("user.email"), Some("cli@example.com"));
        assert_eq!(
            s.get_scoped("user.email", ConfigScope::Global),
            Some("work@corp.example")
        );
    }

    #[test]
    fn parse_empty_and_garbage_never_panics() {
        assert!(ConfigSnapshot::parse(b"").unwrap().entries().is_empty());
        assert!(ConfigSnapshot::parse(b"\0").unwrap().entries().is_empty());
        assert!(ConfigSnapshot::parse(b"\0\0\0")
            .unwrap()
            .entries()
            .is_empty());
        // truncated trailing entry is dropped
        let s = ConfigSnapshot::parse(b"global\0file:x\0a.b\nc\0local\0file:y").unwrap();
        assert_eq!(s.entries().len(), 1);
        // invalid UTF-8 is replaced, not rejected
        let s = ConfigSnapshot::parse(b"bogus\0file:\xff\0a.b\n\xfe\0").unwrap();
        assert_eq!(s.entries()[0].scope, ConfigScope::Unknown);
        assert!(s.entries()[0]
            .value
            .as_deref()
            .unwrap()
            .contains('\u{fffd}'));
        // arbitrary bytes
        for n in 0..=255u8 {
            let raw: Vec<u8> = (0..64).map(|i| n.wrapping_mul(i).wrapping_add(i)).collect();
            let _ = ConfigSnapshot::parse(&raw);
        }
    }

    #[test]
    fn parse_without_trailing_nul() {
        let s = ConfigSnapshot::parse(b"global\0file:x\0user.name\nA").unwrap();
        assert_eq!(s.get("user.name"), Some("A"));
    }
}
