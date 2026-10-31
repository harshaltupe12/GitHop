//! Per-repo pin and global default identity writers (Plan §6, §7.6, §8 C16).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.

use std::path::Path;

use crate::error::Result;
use crate::git::Git;
use crate::gitconfig::{self, git_failed};
use crate::profile::Profile;
use crate::repo::Repo;

/// Local config key that marks a GitHop pin.
pub const PIN_KEY: &str = "githop.profile";

/// Local keys `pin_repo` writes. `PIN_KEY` is written first and removed last, so a
/// half-finished pin can always be undone by `unpin_repo`.
const PINNED_KEYS: [&str; 3] = ["user.name", "user.email", "core.sshCommand"];

/// `ssh -i "<key with forward slashes>" -o IdentitiesOnly=yes` (always quoted, C16).
///
/// git runs `core.sshCommand` through `sh`, so inside the double quotes the
/// characters sh treats specially (`"`, `$`, `` ` ``, `\`) are backslash-escaped.
/// Backslashes in the path become `/` first, so on Windows only `$` and `` ` ``
/// (both legal in file names) ever need escaping.
pub fn ssh_command_for(key: &Path) -> String {
    let path = key.to_string_lossy().replace('\\', "/");
    let mut quoted = String::with_capacity(path.len() + 2);
    for c in path.chars() {
        if matches!(c, '"' | '$' | '`' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    format!("ssh -i \"{quoted}\" -o IdentitiesOnly=yes")
}

/// Extract the `-i` key path from a `core.sshCommand` value (quoted or bare), for
/// comparing a repo's key with its profile's key. `None` when there is no `-i`.
///
/// Understands `-i path`, `-ipath`, `-i"path"`, `-i 'path'`, flags combined with
/// `-i` (`-vi path`), and `-o IdentityFile=path` / `-oIdentityFile path`. The
/// first key found wins. The path is returned unquoted but otherwise as written
/// (no slash normalisation — compare with `paths` helpers).
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
                    return Some(arg.to_string());
                }
                if c == 'o' {
                    if let Some(p) = identity_file_option(arg) {
                        return Some(p);
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
/// `githop.profile` is written first so that even a pin interrupted halfway is
/// recognised (and fully removed) by [`unpin_repo`].
pub fn pin_repo(repo: &Repo, profile: &Profile) -> Result<()> {
    let git = repo.git();
    let root = repo.root();
    gitconfig::set_local(git, root, PIN_KEY, &profile.id)?;
    gitconfig::set_local(git, root, "user.name", &profile.name)?;
    gitconfig::set_local(git, root, "user.email", &profile.email)?;
    gitconfig::set_local(
        git,
        root,
        "core.sshCommand",
        &ssh_command_for(&profile.ssh_key),
    )?;
    Ok(())
}

/// Remove the keys `pin_repo` set — only if `githop.profile` is present.
/// Returns `true` when a pin was removed.
///
/// Only local scope is touched; any other local keys the user set are kept.
pub fn unpin_repo(repo: &Repo) -> Result<bool> {
    let git = repo.git();
    let root = repo.root();
    let args = ["config", "--local", "--get", PIN_KEY];
    let out = git.output(root, &args, None)?;
    match out.status.code() {
        Some(0) => {}
        Some(1) => return Ok(false),
        _ => return Err(git_failed(&args, &out)),
    }
    for key in PINNED_KEYS {
        gitconfig::unset_local(git, root, key)?;
    }
    gitconfig::unset_local(git, root, PIN_KEY)?;
    Ok(true)
}

/// Set global `user.name`, `user.email`, `core.sshCommand` to `profile`.
/// Caller is responsible for `backup::backup_global_gitconfig` first.
pub fn set_default_profile_global(git: &Git, profile: &Profile) -> Result<()> {
    gitconfig::set_global(git, "user.name", &profile.name)?;
    gitconfig::set_global(git, "user.email", &profile.email)?;
    gitconfig::set_global(git, "core.sshCommand", &ssh_command_for(&profile.ssh_key))?;
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
        ];
        for (input, want) in cases {
            assert_eq!(
                ssh_command_for(&PathBuf::from(input)),
                *want,
                "input {input:?}"
            );
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
        ] {
            let cmd = ssh_command_for(Path::new(p));
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
