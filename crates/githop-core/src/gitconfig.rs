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

/// Compare config keys the way git does: section and variable
/// case-insensitive, subsection (the middle part, e.g. a remote name)
/// case-sensitive. `remote.Origin.URL` == `remote.Origin.url` != `remote.origin.url`.
pub fn key_eq(a: &str, b: &str) -> bool {
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

    /// Only the repo's own `.git/config` (`--local`), one spawn. Entries have
    /// scope `local`.
    pub fn read_local(git: &Git, repo_root: &Path) -> Result<Self> {
        let out = git.run(
            repo_root,
            &[
                "config",
                "--local",
                "--list",
                "--show-origin",
                "--show-scope",
                "-z",
            ],
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

    /// Last value of `key` set at exactly `scope`, with its origin.
    pub fn get_scoped_entry(&self, key: &str, scope: ConfigScope) -> Option<&ConfigEntry> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.scope == scope && key_eq(&e.key, key))
    }

    /// URL of remote `name` as written (no spawn). With several
    /// `remote.<name>.url` values this is the **first** one — the URL git
    /// fetches from (the others are extra push URLs).
    pub fn remote_url(&self, name: &str) -> Option<&str> {
        let key = format!("remote.{name}.url");
        self.entries
            .iter()
            .find(|e| key_eq(&e.key, &key))
            .map(|e| e.value.as_deref().unwrap_or("true"))
    }

    /// Apply `url.<base>.insteadOf` rewriting to `url` like git does for fetch
    /// URLs: the longest matching `insteadOf` prefix is replaced by its `<base>`
    /// (first defined wins a tie). Returns `None` when no rule matches. No spawn.
    pub fn rewrite_url(&self, url: &str) -> Option<String> {
        let mut best: Option<(usize, &str)> = None;
        for e in &self.entries {
            let Some((section, rest)) = e.key.split_once('.') else {
                continue;
            };
            if !section.eq_ignore_ascii_case("url") {
                continue;
            }
            let Some((base, var)) = rest.rsplit_once('.') else {
                continue;
            };
            if !var.eq_ignore_ascii_case("insteadof") {
                continue;
            }
            let Some(prefix) = e.value.as_deref().filter(|p| !p.is_empty()) else {
                continue;
            };
            if url.starts_with(prefix) && best.is_none_or(|(len, _)| prefix.len() > len) {
                best = Some((prefix.len(), base));
            }
        }
        best.map(|(len, base)| format!("{base}{}", &url[len..]))
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

/// Set a global value. Caller must back up first (`backup` module).
///
/// Writes with `git config --file <Git::global_config_path()>` rather than
/// `--global`, so the file written is exactly the file `backup` copied (git's
/// own `~/.gitconfig` / XDG choice is mirrored by `global_config_path`).
pub fn set_global(git: &Git, key: &str, value: &str) -> Result<()> {
    let file = git.global_config_path()?;
    set_file(git, &file, key, value)
}

/// Add one more value to a (multi-valued) global key (`--add`). Caller backs up first.
pub fn add_global(git: &Git, key: &str, value: &str) -> Result<()> {
    let file = git.global_config_path()?;
    let file_s = file.to_string_lossy();
    git.run(
        &global_cwd(git),
        &["config", "--file", &file_s, "--add", key, value],
    )
    .map(drop)
}

/// Unset every value of a global key; a missing key is not an error.
/// Same target file as [`set_global`].
pub fn unset_global(git: &Git, key: &str) -> Result<()> {
    let file = git.global_config_path()?;
    unset_file(git, &file, key)
}

/// `git config --file <file> --replace-all <key> <value>`.
pub fn set_file(git: &Git, file: &Path, key: &str, value: &str) -> Result<()> {
    let file_s = file.to_string_lossy();
    git.run(
        &global_cwd(git),
        &["config", "--file", &file_s, "--replace-all", key, value],
    )
    .map(drop)
}

/// `git config --file <file> --unset-all <key>`; a missing key (or missing
/// file) is not an error.
pub fn unset_file(git: &Git, file: &Path, key: &str) -> Result<()> {
    if std::fs::symlink_metadata(file).is_err() {
        return Ok(());
    }
    let file_s = file.to_string_lossy();
    let args = ["config", "--file", &file_s, "--unset-all", key];
    let out = git.output(&global_cwd(git), &args, None)?;
    if out.status.success() || out.status.code() == Some(EXIT_KEY_MISSING) {
        Ok(())
    } else {
        Err(git_failed(&args, &out))
    }
}

/// Read one value from a specific config file (`git config --file <file> --get
/// <key>`), e.g. a backup. `None` when unset or when the file doesn't exist.
/// With several values the last one wins. Includes are not followed.
pub fn get_file(git: &Git, file: &Path, key: &str) -> Result<Option<String>> {
    Ok(get_all_file(git, file, key)?.pop())
}

/// Every value of `key` in a specific config file, in file order (`--get-all
/// -z`, so values containing newlines survive). Empty when unset or no file.
pub fn get_all_file(git: &Git, file: &Path, key: &str) -> Result<Vec<String>> {
    if !file.is_file() {
        return Ok(Vec::new());
    }
    let file_s = file.to_string_lossy();
    let args = ["config", "--file", &file_s, "-z", "--get-all", key];
    let out = git.output(&global_cwd(git), &args, None)?;
    if out.status.success() {
        return Ok(split_nul_values(&out.stdout));
    }
    if out.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    Err(git_failed(&args, &out))
}

/// Every entry of one config file, in file order, **without** following its
/// includes: `include.path` / `includeIf.<cond>.path` lines show up as entries
/// themselves (`git config --file <file> --list --show-origin --show-scope -z`).
/// Empty when the file doesn't exist.
pub fn list_file(git: &Git, file: &Path) -> Result<ConfigSnapshot> {
    if !file.is_file() {
        return Ok(ConfigSnapshot::default());
    }
    let file_s = file.to_string_lossy();
    let out = git.run(
        &global_cwd(git),
        &[
            "config",
            "--file",
            &file_s,
            "--list",
            "--show-origin",
            "--show-scope",
            "-z",
        ],
    )?;
    ConfigSnapshot::parse(&out)
}

/// Like [`get_all_file`], but follows the file's own `[include]`s
/// (`--includes`). Conditional includes can't match outside a repo.
pub fn get_all_file_includes(git: &Git, file: &Path, key: &str) -> Result<Vec<String>> {
    if !file.is_file() {
        return Ok(Vec::new());
    }
    let file_s = file.to_string_lossy();
    let args = [
        "config",
        "--file",
        &file_s,
        "--includes",
        "-z",
        "--get-all",
        key,
    ];
    let out = git.output(&global_cwd(git), &args, None)?;
    if out.status.success() {
        return Ok(split_nul_values(&out.stdout));
    }
    if out.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    Err(git_failed(&args, &out))
}

/// Escape `s` so a POSIX extended regex (git's value pattern) matches it literally.
pub fn regex_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(
            c,
            '.' | '[' | ']' | '{' | '}' | '(' | ')' | '*' | '+' | '?' | '^' | '$' | '|' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Remove only the values of `key` in `file` that are exactly `value`
/// (`--unset-all <key> ^<value>$`); other values stay where they are. A missing
/// key or file is not an error. Caller backs up first.
pub fn unset_file_value(git: &Git, file: &Path, key: &str, value: &str) -> Result<()> {
    if std::fs::symlink_metadata(file).is_err() {
        return Ok(());
    }
    let file_s = file.to_string_lossy();
    let pattern = format!("^{}$", regex_literal(value));
    let args = ["config", "--file", &file_s, "--unset-all", key, &pattern];
    let out = git.output(&global_cwd(git), &args, None)?;
    if out.status.success() || out.status.code() == Some(EXIT_KEY_MISSING) {
        Ok(())
    } else {
        Err(git_failed(&args, &out))
    }
}

/// Replace, in place, the values of `key` in `file` that are exactly `old`
/// with `new` (`--replace-all <key> <new> ^<old>$`). Caller backs up first.
pub fn replace_file_value(git: &Git, file: &Path, key: &str, old: &str, new: &str) -> Result<()> {
    let file_s = file.to_string_lossy();
    let pattern = format!("^{}$", regex_literal(old));
    git.run(
        &global_cwd(git),
        &[
            "config",
            "--file",
            &file_s,
            "--replace-all",
            key,
            new,
            &pattern,
        ],
    )
    .map(drop)
}

/// Split successful `-z` value output (`value\0value\0`) into strings. Only
/// called on exit 0, so there is at least one value (a lone `\0` is one empty value).
fn split_nul_values(raw: &[u8]) -> Vec<String> {
    let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
    raw.split(|b| *b == 0)
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .collect()
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

    /// Review #7: git fetches from the first `remote.<name>.url`.
    #[test]
    fn remote_url_uses_first_value() {
        let s = ConfigSnapshot::from_entries(vec![
            e(
                ConfigScope::Local,
                "remote.origin.url",
                "git@github.com:acme/x.git",
            ),
            e(
                ConfigScope::Local,
                "remote.origin.url",
                "git@github.com:me/x.git",
            ),
        ]);
        assert_eq!(s.remote_url("origin"), Some("git@github.com:acme/x.git"));
        assert_eq!(s.get_all("remote.origin.url").len(), 2);
    }

    #[test]
    fn rewrite_url_longest_insteadof_wins() {
        let s = ConfigSnapshot::from_entries(vec![
            e(ConfigScope::Global, "url.git@github.com:.insteadof", "gh:"),
            e(
                ConfigScope::Global,
                "url.git@github-office:Acme/.insteadOf",
                "gh:acme/",
            ),
            e(
                ConfigScope::Global,
                "url.https://mirror.example/.insteadOf",
                "x",
            ),
            e(ConfigScope::Global, "url.unused.pushInsteadOf", "gh:"),
            e(ConfigScope::Global, "url.empty.insteadOf", ""),
        ]);
        assert_eq!(
            s.rewrite_url("gh:me/repo").as_deref(),
            Some("git@github.com:me/repo")
        );
        assert_eq!(
            s.rewrite_url("gh:acme/api").as_deref(),
            Some("git@github-office:Acme/api")
        );
        assert_eq!(s.rewrite_url("https://github.com/a/b"), None);
    }

    #[test]
    fn scoped_entry_and_key_eq() {
        let s = ConfigSnapshot::from_entries(vec![
            e(ConfigScope::Global, "user.email", "g@x"),
            e(ConfigScope::Local, "user.email", "l@x"),
        ]);
        let g = s
            .get_scoped_entry("User.Email", ConfigScope::Global)
            .unwrap();
        assert_eq!(g.value.as_deref(), Some("g@x"));
        assert!(s
            .get_scoped_entry("user.email", ConfigScope::Worktree)
            .is_none());
        assert!(key_eq("remote.Origin.URL", "REMOTE.Origin.url"));
        assert!(!key_eq("remote.Origin.url", "remote.origin.url"));
        assert!(key_eq(
            "url.https://a.b/.insteadOf",
            "url.https://a.b/.insteadof"
        ));
    }

    #[test]
    fn nul_values_split() {
        assert_eq!(split_nul_values(b"a\0b\nc\0"), vec!["a", "b\nc"]);
        assert_eq!(split_nul_values(b"\0"), vec![""]);
    }

    #[test]
    fn regex_literal_escapes_ere_metacharacters() {
        assert_eq!(regex_literal("C:/a b/hooks"), "C:/a b/hooks");
        assert_eq!(
            regex_literal(r"C:\x.y(1)+[a]$^|?*{2}"),
            r"C:\\x\.y\(1\)\+\[a\]\$\^\|\?\*\{2\}"
        );
    }

    #[test]
    fn parse_without_trailing_nul() {
        let s = ConfigSnapshot::parse(b"global\0file:x\0user.name\nA").unwrap();
        assert_eq!(s.get("user.name"), Some("A"));
    }
}
