//! Per-repo pin and global default identity writers (Plan §6, §7.6, §8 C16).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.

use std::path::Path;

use crate::error::Result;
use crate::git::Git;
use crate::gitconfig::{self, ConfigSnapshot};
use crate::profile::Profile;
use crate::repo::Repo;

/// Local config key that marks a GitHop pin.
pub const PIN_KEY: &str = "githop.profile";

/// Local keys `pin_repo` writes. `PIN_KEY` is written first and removed last, so a
/// half-finished pin can always be undone by `unpin_repo`.
const PINNED_KEYS: [&str; 3] = ["user.name", "user.email", "core.sshCommand"];

/// Where `pin_repo` keeps the user's own value of each of [`PINNED_KEYS`]
/// (same order), restored by `unpin_repo`.
pub const SAVED_KEYS: [&str; 3] = [
    "githop.saved-name",
    "githop.saved-email",
    "githop.saved-sshcommand",
];

/// Keys GitHop writes for a profile that signs its commits (ADR 0008), in the
/// repo (on pin) and globally (default profile).
pub const SIGN_KEYS: [&str; 4] = [
    "gpg.format",
    "user.signingkey",
    "commit.gpgsign",
    "gpg.ssh.program",
];

/// Where `pin_repo` keeps the user's own value of each of [`SIGN_KEYS`].
pub const SAVED_SIGN_KEYS: [&str; 4] = [
    "githop.saved-gpgformat",
    "githop.saved-signingkey",
    "githop.saved-gpgsign",
    "githop.saved-gpgsshprogram",
];

/// Marks that the [`SIGN_KEYS`] in this config file were written by GitHop.
pub const SIGNING_MARK: &str = "githop.signing";

/// Values for [`SIGN_KEYS`] (same order): SSH signing with the profile's key.
///
/// `gpg.ssh.program` is Windows' own `ssh-keygen` when present: like
/// `core.sshCommand` (ADR 0006), only Windows OpenSSH talks to the Windows
/// ssh-agent, so a remembered passphrase signs without a prompt (VS Code too).
pub fn signing_values(profile: &Profile) -> [String; 4] {
    let program = crate::ssh::windows_openssh_tool("ssh-keygen")
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| "ssh-keygen".to_string());
    let mut public = profile.ssh_key.as_os_str().to_owned();
    public.push(".pub");
    [
        "ssh".to_string(),
        public.to_string_lossy().replace('\\', "/"),
        "true".to_string(),
        program,
    ]
}

/// `ssh -i "<key with forward slashes>" -o IdentitiesOnly=yes` (always quoted, C16).
///
/// git runs `core.sshCommand` through `sh`, so inside the double quotes the
/// characters sh treats specially (`"`, `$`, `` ` ``, `\`) are backslash-escaped.
/// Backslashes in the path become `/` first, so on Windows only `$` and `` ` ``
/// (both legal in file names) ever need escaping.
///
/// OpenSSH expands `%` tokens (`%d`, `%u`, …) in `-i` paths, so a literal `%`
/// is written as `%%` ([`escape_ssh_percent`]); [`key_from_ssh_command`] undoes it.
///
/// The program is Windows' own OpenSSH (`"C:/Windows/System32/OpenSSH/ssh.exe"`,
/// quoted) when it exists, else plain `ssh` (ADR 0006): only Windows OpenSSH
/// talks to the Windows ssh-agent, so a remembered passphrase is used by
/// terminal git and by VS Code's Git (same git config) without a prompt.
/// `IdentitiesOnly=yes` with `-i` still lets ssh use the agent's copy of that
/// key.
pub fn ssh_command_for(key: &Path) -> String {
    ssh_command_with(crate::ssh::git_ssh_program().as_deref(), key)
}

/// [`ssh_command_for`] with an explicit ssh program (`None` = plain `ssh`). Pure.
pub fn ssh_command_with(program: Option<&Path>, key: &Path) -> String {
    let path = escape_ssh_percent(&key.to_string_lossy().replace('\\', "/"));
    let program = match program {
        Some(p) => format!(
            "\"{}\"",
            sh_quote_inner(&p.to_string_lossy().replace('\\', "/"))
        ),
        None => "ssh".to_string(),
    };
    format!(
        "{program} -i \"{}\" -o IdentitiesOnly=yes",
        sh_quote_inner(&path)
    )
}

/// Backslash-escape what `sh` treats specially inside double quotes.
fn sh_quote_inner(s: &str) -> String {
    let mut quoted = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(c, '"' | '$' | '`' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted
}

/// `%` → `%%`, so OpenSSH's token expansion of `-i` / `IdentityFile` yields the
/// literal path (review #6).
pub fn escape_ssh_percent(path: &str) -> String {
    path.replace('%', "%%")
}

/// Undo [`escape_ssh_percent`]: `%%` → `%`. Other `%` tokens are left as written.
pub fn unescape_ssh_percent(path: &str) -> String {
    path.replace("%%", "%")
}

/// Extract the `-i` key path from a `core.sshCommand` value (quoted or bare), for
/// comparing a repo's key with its profile's key. `None` when there is no `-i`.
///
/// Understands `-i path`, `-ipath`, `-i"path"`, `-i 'path'`, flags combined with
/// `-i` (`-vi path`), and `-o IdentityFile=path` / `-oIdentityFile path`. The
/// first key found wins. The path is returned unquoted, with ssh's `%%`
/// escape undone, but otherwise as written (no slash normalisation — compare
/// with `paths` helpers).
pub fn key_from_ssh_command(cmd: &str) -> Option<String> {
    /// ssh options that take an argument (OpenSSH `getopt` string).
    const ARG_FLAGS: &str = "BbcDEeFIiJLlmOoPpQRSWw";
    let words = shell_words(cmd);
    let mut i = 1; // skip the program
    while i < words.len() {
        let w = &words[i];
        if w == "--" {
            break;
        }
        if let Some(flags) = w.strip_prefix('-').filter(|f| !f.is_empty()) {
            for (pos, c) in flags.char_indices() {
                if !ARG_FLAGS.contains(c) {
                    continue;
                }
                let rest = &flags[pos + c.len_utf8()..];
                let arg = if rest.is_empty() {
                    i += 1;
                    words.get(i)?.as_str()
                } else {
                    rest
                };
                if c == 'i' {
                    return Some(unescape_ssh_percent(arg));
                }
                if c == 'o' {
                    if let Some(p) = identity_file_option(arg) {
                        return Some(unescape_ssh_percent(&p));
                    }
                }
                break;
            }
        }
        i += 1;
    }
    None
}

/// `IdentityFile=path` / `IdentityFile path` (keyword case-insensitive) → path.
fn identity_file_option(opt: &str) -> Option<String> {
    let opt = opt.trim();
    let split = opt.find(|c: char| c == '=' || c.is_whitespace())?;
    let (key, value) = opt.split_at(split);
    if !key.eq_ignore_ascii_case("IdentityFile") {
        return None;
    }
    let value = value
        .trim_start_matches(|c: char| c.is_whitespace())
        .strip_prefix('=')
        .unwrap_or(value.trim_start())
        .trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    (!value.is_empty()).then(|| value.to_string())
}

/// Split a command line the way `sh` would, with one Windows-friendly change: a
/// backslash outside quotes is literal unless it escapes whitespace or a quote
/// (so a bare `C:\Users\me\key` survives). Unterminated quotes run to the end.
fn shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur: Option<String> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if let Some(w) = cur.take() {
                    words.push(w);
                }
            }
            '\'' => {
                let w = cur.get_or_insert_with(String::new);
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    w.push(c);
                }
            }
            '"' => {
                let w = cur.get_or_insert_with(String::new);
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.peek().copied() {
                            Some(n @ ('$' | '`' | '"' | '\\')) => {
                                w.push(n);
                                chars.next();
                            }
                            Some('\n') => {
                                chars.next();
                            }
                            _ => w.push('\\'),
                        },
                        c => w.push(c),
                    }
                }
            }
            '\\' => {
                let w = cur.get_or_insert_with(String::new);
                match chars.peek().copied() {
                    Some(n) if n.is_whitespace() || n == '"' || n == '\'' => {
                        w.push(n);
                        chars.next();
                    }
                    _ => w.push('\\'),
                }
            }
            c => cur.get_or_insert_with(String::new).push(c),
        }
    }
    if let Some(w) = cur {
        words.push(w);
    }
    words
}

/// Set local `user.name`, `user.email`, `core.sshCommand`, `githop.profile`.
///
/// The user's own local values are kept (review 2026-10-31 #3): when the repo
/// is not pinned yet, any existing local `user.name` / `user.email` /
/// `core.sshCommand` is first copied to `githop.saved-name` /
/// `githop.saved-email` / `githop.saved-sshcommand` (an existing saved value
/// is never overwritten), and [`unpin_repo`] puts it back. Values are saved
/// even when they equal the profile's, so unpinning never drops a value the
/// user set. Re-pinning an already pinned repo saves nothing (the current
/// values are GitHop's own).
///
/// Order: saved copies → `githop.profile` → values. A pin interrupted halfway
/// is still recognised (and fully undone) by [`unpin_repo`]. Spawns: one
/// `git config --local --list` plus one per key written.
pub fn pin_repo(repo: &Repo, profile: &Profile) -> Result<()> {
    let git = repo.git();
    let root = repo.root();
    let local = ConfigSnapshot::read_local(git, root)?;
    if local.get(PIN_KEY).is_none() {
        for (key, saved) in PINNED_KEYS.iter().zip(SAVED_KEYS) {
            if let Some(value) = local.get(key) {
                if local.get(saved).is_none() {
                    gitconfig::set_local(git, root, saved, value)?;
                }
            }
        }
    }
    gitconfig::set_local(git, root, PIN_KEY, &profile.id)?;
    gitconfig::set_local(git, root, "user.name", &profile.name)?;
    gitconfig::set_local(git, root, "user.email", &profile.email)?;
    gitconfig::set_local(
        git,
        root,
        "core.sshCommand",
        &ssh_command_for(&profile.ssh_key),
    )?;
    if profile.sign_commits {
        sign_local(git, root, &local, profile)?;
    } else if local.get(SIGNING_MARK).is_some() {
        unsign_local(git, root, &local)?;
    }
    Ok(())
}

/// Turn on SSH signing in the repo. The user's own values are saved once
/// (like the identity keys) and come back with [`unsign_local`].
fn sign_local(git: &Git, root: &Path, local: &ConfigSnapshot, profile: &Profile) -> Result<()> {
    if local.get(SIGNING_MARK).is_none() {
        for (key, saved) in SIGN_KEYS.iter().zip(SAVED_SIGN_KEYS) {
            if let (Some(value), None) = (local.get(key), local.get(saved)) {
                gitconfig::set_local(git, root, saved, value)?;
            }
        }
        gitconfig::set_local(git, root, SIGNING_MARK, "true")?;
    }
    for (key, value) in SIGN_KEYS.iter().zip(signing_values(profile)) {
        gitconfig::set_local(git, root, key, &value)?;
    }
    Ok(())
}

/// Undo [`sign_local`]: the user's own values back, GitHop's removed.
fn unsign_local(git: &Git, root: &Path, local: &ConfigSnapshot) -> Result<()> {
    for (key, saved) in SIGN_KEYS.iter().zip(SAVED_SIGN_KEYS) {
        match local.get(saved) {
            Some(value) => {
                gitconfig::set_local(git, root, key, value)?;
                gitconfig::unset_local(git, root, saved)?;
            }
            None => gitconfig::unset_local(git, root, key)?,
        }
    }
    gitconfig::unset_local(git, root, SIGNING_MARK)
}

/// Remove the keys `pin_repo` set — only if `githop.profile` is present.
/// Returns `true` when a pin was removed.
///
/// A key with a `githop.saved-*` copy gets the user's value back (and the copy
/// is removed); a key without one is unset. Only local scope is touched; any
/// other local keys the user set are kept. `githop.profile` is removed last.
pub fn unpin_repo(repo: &Repo) -> Result<bool> {
    let git = repo.git();
    let root = repo.root();
    let local = ConfigSnapshot::read_local(git, root)?;
    if local.get(PIN_KEY).is_none() {
        return Ok(false);
    }
    for (key, saved) in PINNED_KEYS.iter().zip(SAVED_KEYS) {
        match local.get(saved) {
            Some(value) => {
                gitconfig::set_local(git, root, key, value)?;
                gitconfig::unset_local(git, root, saved)?;
            }
            None => gitconfig::unset_local(git, root, key)?,
        }
    }
    if local.get(SIGNING_MARK).is_some() {
        unsign_local(git, root, &local)?;
    }
    gitconfig::unset_local(git, root, PIN_KEY)?;
    Ok(true)
}

/// Set global `user.name`, `user.email`, `core.sshCommand` to `profile`, plus
/// the [`SIGN_KEYS`] when it signs its commits. Switching to a profile that
/// doesn't sign removes only signing keys GitHop wrote ([`SIGNING_MARK`]), so a
/// user's own global signing setup is never touched. Wipe restores the
/// originals from the backup (`wipe::MANAGED_GLOBAL_KEYS`).
/// Caller is responsible for `backup::backup_global_gitconfig` first.
pub fn set_default_profile_global(git: &Git, profile: &Profile) -> Result<()> {
    gitconfig::set_global(git, "user.name", &profile.name)?;
    gitconfig::set_global(git, "user.email", &profile.email)?;
    gitconfig::set_global(git, "core.sshCommand", &ssh_command_for(&profile.ssh_key))?;
    if profile.sign_commits {
        for (key, value) in SIGN_KEYS.iter().zip(signing_values(profile)) {
            gitconfig::set_global(git, key, &value)?;
        }
        gitconfig::set_global(git, SIGNING_MARK, "true")?;
    } else if gitconfig::get_global(git, SIGNING_MARK)?.is_some() {
        for key in SIGN_KEYS {
            gitconfig::unset_global(git, key)?;
        }
        gitconfig::unset_global(git, SIGNING_MARK)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ssh_command_quoting_table() {
        let cases: &[(&str, &str)] = &[
            (
                r"C:\Users\me\.ssh\id_ed25519",
                r#"ssh -i "C:/Users/me/.ssh/id_ed25519" -o IdentitiesOnly=yes"#,
            ),
            (
                r"C:\Users\Jane Doe\.ssh\id_work",
                r#"ssh -i "C:/Users/Jane Doe/.ssh/id_work" -o IdentitiesOnly=yes"#,
            ),
            (
                "C:/Users/Jörg Ünï/.ssh/日本",
                r#"ssh -i "C:/Users/Jörg Ünï/.ssh/日本" -o IdentitiesOnly=yes"#,
            ),
            (
                r"\\server\share\keys\id",
                r#"ssh -i "//server/share/keys/id" -o IdentitiesOnly=yes"#,
            ),
            (
                r"C:\Users\a$b`c\key",
                r#"ssh -i "C:/Users/a\$b\`c/key" -o IdentitiesOnly=yes"#,
            ),
            // Review #6: ssh expands %d/%u/… in -i; a literal % is %%.
            (
                r"C:\Users\me\100%\id_%d",
                r#"ssh -i "C:/Users/me/100%%/id_%%d" -o IdentitiesOnly=yes"#,
            ),
        ];
        for (input, want) in cases {
            assert_eq!(
                ssh_command_with(None, &PathBuf::from(input)),
                *want,
                "input {input:?}"
            );
        }
    }

    /// ADR 0006: Windows OpenSSH (where the ssh-agent lives) is quoted, with
    /// forward slashes, and the key is still found by `key_from_ssh_command`.
    #[test]
    fn ssh_command_with_windows_openssh_program() {
        let cmd = ssh_command_with(
            Some(Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe")),
            Path::new(r"C:\Users\Jane Doe\.ssh\githop_personal"),
        );
        assert_eq!(
            cmd,
            r#""C:/Windows/System32/OpenSSH/ssh.exe" -i "C:/Users/Jane Doe/.ssh/githop_personal" -o IdentitiesOnly=yes"#
        );
        assert_eq!(
            key_from_ssh_command(&cmd).as_deref(),
            Some("C:/Users/Jane Doe/.ssh/githop_personal")
        );
        // What pins write on this machine: Windows OpenSSH when it exists.
        let live = ssh_command_for(Path::new("C:/k/id"));
        match crate::ssh::git_ssh_program() {
            Some(_) => assert!(
                live.starts_with('"') && live.contains("/OpenSSH/ssh.exe\" -i \"C:/k/id\""),
                "{live}"
            ),
            None => assert_eq!(live, r#"ssh -i "C:/k/id" -o IdentitiesOnly=yes"#),
        }
    }

    #[test]
    fn ssh_command_round_trips() {
        for p in [
            r"C:\Users\me\.ssh\id_ed25519",
            r"C:\Users\Jane Doe\.ssh\id work",
            "C:/Users/Jörg Ünï/.ssh/日本",
            r"\\server\share\keys\id",
            r"C:\Users\a$b`c\key",
            r#"/home/x/we"ird"#,
            r"C:\Users\me\100%\id_%d%%",
        ] {
            let cmd = ssh_command_with(None, Path::new(p));
            assert_eq!(
                key_from_ssh_command(&cmd).as_deref(),
                Some(p.replace('\\', "/").as_str()),
                "cmd {cmd:?}"
            );
        }
    }

    #[test]
    fn key_extraction_table() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                r#"ssh -i "C:/a b/key" -o IdentitiesOnly=yes"#,
                Some("C:/a b/key"),
            ),
            ("ssh -i C:/keys/id", Some("C:/keys/id")),
            (r"ssh -i C:\Users\me\.ssh\id", Some(r"C:\Users\me\.ssh\id")),
            (
                r#"ssh -i "C:\Users\Jane Doe\.ssh\id""#,
                Some(r"C:\Users\Jane Doe\.ssh\id"),
            ),
            (r#"ssh -i"C:/a b/key""#, Some("C:/a b/key")),
            ("ssh -iC:/k", Some("C:/k")),
            ("ssh -i 'C:/a b/key'", Some("C:/a b/key")),
            (r"ssh -i C:/a\ b/key", Some("C:/a b/key")),
            ("ssh -v -i ~/.ssh/id", Some("~/.ssh/id")),
            ("ssh -vi ~/.ssh/id", Some("~/.ssh/id")),
            ("ssh -p 2222 -i /k", Some("/k")),
            ("ssh -p2222 -i /k", Some("/k")),
            ("ssh -o IdentityFile=/k/id", Some("/k/id")),
            ("ssh -oIdentityFile=/k/id", Some("/k/id")),
            (r#"ssh -o "IdentityFile C:/a b/id""#, Some("C:/a b/id")),
            (r#"ssh -o 'identityfile="C:/a b/id"'"#, Some("C:/a b/id")),
            ("ssh -o IdentitiesOnly=yes -i /k", Some("/k")),
            (
                r#""C:/Program Files/OpenSSH/ssh.exe" -i "D:/k""#,
                Some("D:/k"),
            ),
            ("ssh -i //server/share/id", Some("//server/share/id")),
            ("ssh -i \"C:/Users/Jörg/日本\"", Some("C:/Users/Jörg/日本")),
            ("ssh", None),
            ("", None),
            ("ssh -o IdentitiesOnly=yes", None),
            ("ssh -i", None),
            ("ssh -F /cfg host", None),
            ("ssh -- -i /k", None),
            ("ssh -i \"unterminated", Some("unterminated")),
            ("ssh -i C:/a%%b/id", Some("C:/a%b/id")),
            ("ssh -o IdentityFile=C:/a%%b", Some("C:/a%b")),
        ];
        for (input, want) in cases {
            assert_eq!(
                key_from_ssh_command(input).as_deref(),
                *want,
                "input {input:?}"
            );
        }
    }
}
