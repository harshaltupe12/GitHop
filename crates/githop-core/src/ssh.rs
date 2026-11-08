//! SSH helpers: key generation, ACLs, ssh config conflicts, connection test (Plan §8 C10/C11, §9.2).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! "ssh & scan" task. All processes are spawned with argument arrays.
//!
//! GitHop never opens a socket itself. The only network action here is
//! [`test_connection`], which spawns the user's own `ssh` and only runs when the
//! user asks for it ("Test connection").

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::git::{find_on_path, Git};

/// A `Host` block in `~/.ssh/config` that can make SSH offer the wrong key for GitHub (C10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConflict {
    /// The `Host` patterns line, e.g. `github.com` or `*`.
    pub host_pattern: String,
    /// The `IdentityFile` value inside that block.
    pub identity_file: String,
    /// 1-based line number of the `IdentityFile` directive.
    pub line: usize,
}

/// Result of `ssh -T git@github.com`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ConnectionTest {
    /// GitHub answered "Hi <user>!".
    Connected { user: String },
    /// Key was rejected (`Permission denied (publickey)`).
    Denied,
    /// Anything else (timeout, DNS, ssh missing…), with a short reason.
    Failed { reason: String },
}

/// Windows OpenSSH agent service state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Running,
    Stopped,
    Unknown,
}

/// The host GitHop cares about in ssh config.
const GITHUB_HOST: &str = "github.com";

/// `~/.ssh` under the home dir git/ssh would use.
pub fn ssh_dir(git: &Git) -> Option<PathBuf> {
    git.home_dir().map(|h| h.join(".ssh"))
}

/// Default key path for a profile: `~/.ssh/githop_<id>`.
pub fn default_key_path(git: &Git, profile_id: &str) -> Option<PathBuf> {
    ssh_dir(git).map(|d| d.join(format!("githop_{profile_id}")))
}

/// Find `Host` blocks matching `github.com` (incl. `*`, `github.com*` patterns) that set
/// `IdentityFile`. Pure; never panics. Ignores comments, handles `Host a b c` lists,
/// `key=value` and `key value` forms, case-insensitive keywords, `Match` blocks end a Host block.
///
/// Matching follows OpenSSH: a `Host` line applies when at least one plain pattern
/// matches `github.com` (globs `*` and `?`, case-insensitive) and no `!negated`
/// pattern matches. Directives before the first `Host`/`Match` line apply to every
/// host, so an `IdentityFile` there is reported with `host_pattern` `*`.
///
/// Not evaluated (by design, this is a static check):
/// - `Include` is **not** followed; only the given text is checked.
/// - `Match` blocks are skipped entirely (their criteria need runtime context).
/// - Aliases such as `Host github-personal` + `HostName github.com` are **not**
///   conflicts: ssh matches `Host` against the name on the command line, so they
///   never apply to `git@github.com` traffic. Scoped aliases are the recommended fix.
pub fn find_github_conflicts(config: &str) -> Vec<SshConflict> {
    // Current section: None = inside a Match block (ignored);
    // Some((pattern_line, applies_to_github)).
    let mut section: Option<(String, bool)> = Some(("*".to_string(), true));
    let mut out = Vec::new();

    let config = config.strip_prefix('\u{feff}').unwrap_or(config);
    for (idx, raw_line) in config.lines().enumerate() {
        let Some((keyword, value)) = split_directive(raw_line) else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            let patterns = tokenize(&value);
            let applies = host_line_matches(&patterns, GITHUB_HOST);
            section = Some((patterns.join(" "), applies));
        } else if keyword.eq_ignore_ascii_case("match") {
            section = None;
        } else if keyword.eq_ignore_ascii_case("identityfile") {
            if let Some((pattern, true)) = &section {
                if let Some(file) = tokenize(&value).into_iter().next() {
                    out.push(SshConflict {
                        host_pattern: pattern.clone(),
                        identity_file: file,
                        line: idx + 1,
                    });
                }
            }
        }
    }
    out
}

/// Split a config line into `(keyword, rest)`. Handles `key value`, `key=value`,
/// `key = value`, leading whitespace and `#` comments. `None` for blank/comment lines.
fn split_directive(line: &str) -> Option<(String, String)> {
    let line = strip_comment(line).trim();
    if line.is_empty() {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = &line[..end];
    if keyword.is_empty() {
        return None;
    }
    let mut rest = line[end..].trim_start();
    if let Some(r) = rest.strip_prefix('=') {
        rest = r.trim_start();
    }
    Some((keyword.to_string(), rest.trim_end().to_string()))
}

/// Drop a `#` comment: a whole-line comment, or a `#` that starts a token outside quotes.
fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    let mut prev_ws = true;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '#' if !in_quote && prev_ws => return &line[..i],
            _ => {}
        }
        prev_ws = c.is_whitespace();
    }
    line
}

/// Split a value into whitespace-separated tokens; `"double quotes"` group a token.
fn tokenize(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has_token = false;
    for c in value.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

/// OpenSSH `Host` semantics: any positive match and no negated match.
/// Patterns may also be comma-separated within one token.
fn host_line_matches(patterns: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let mut positive = false;
    for pat in patterns.iter().flat_map(|p| p.split(',')) {
        let pat = pat.trim();
        if pat.is_empty() {
            continue;
        }
        if let Some(neg) = pat.strip_prefix('!') {
            if glob_match(&neg.to_ascii_lowercase(), &host) {
                return false;
            }
        } else if glob_match(&pat.to_ascii_lowercase(), &host) {
            positive = true;
        }
    }
    positive
}

/// Glob with `*` (any run) and `?` (one char). Iterative, no recursion blow-up.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_ti = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            star_ti = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Read `~/.ssh/config` (missing file → empty list) and run [`find_github_conflicts`].
pub fn ssh_config_conflicts(git: &Git) -> Result<Vec<SshConflict>> {
    let Some(dir) = ssh_dir(git) else {
        return Ok(Vec::new());
    };
    let path = dir.join("config");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(find_github_conflicts(&String::from_utf8_lossy(&bytes))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(CoreError::io(path, e)),
    }
}

/// Locate a tool on PATH; on Windows also try `%SystemRoot%\System32`.
fn locate_tool(name: &str) -> Result<PathBuf> {
    if let Some(p) = find_on_path(name) {
        return Ok(p);
    }
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let sys = PathBuf::from(root).join("System32");
            for candidate in [
                sys.join(format!("{name}.exe")),
                sys.join("OpenSSH").join(format!("{name}.exe")),
            ] {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(CoreError::io(
        name,
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{name} was not found on PATH"),
        ),
    ))
}

/// Hide the console window for background helper processes (tray app).
fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// `<key>.pub` (appended, not an extension swap — key names may contain dots).
fn pub_path(key: &Path) -> PathBuf {
    let mut s: OsString = key.as_os_str().to_owned();
    s.push(".pub");
    PathBuf::from(s)
}

/// Spawn `ssh-keygen -t ed25519 -f <key> -C <comment>` with inherited stdio so the
/// passphrase prompt goes straight to the user (GitHop never sees it).
/// `no_passphrase` adds `-N ""`. Refuses with `CoreError::Io` (`AlreadyExists`) if the key exists.
/// Creates the parent dir. Calls [`restrict_key_acl`] afterwards.
pub fn keygen(key: &Path, comment: &str, no_passphrase: bool) -> Result<()> {
    for existing in [key.to_path_buf(), pub_path(key)] {
        if std::fs::symlink_metadata(&existing).is_ok() {
            return Err(CoreError::io(
                existing,
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a key file already exists here; GitHop never overwrites keys",
                ),
            ));
        }
    }
    if let Some(parent) = key.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    let exe = locate_tool("ssh-keygen")?;
    let mut cmd = Command::new(&exe);
    cmd.arg("-t")
        .arg("ed25519")
        .arg("-f")
        .arg(key)
        .arg("-C")
        .arg(comment);
    if no_passphrase {
        // An empty argument, not a shell-quoted `""`.
        cmd.arg("-N").arg("");
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = cmd.status().map_err(|e| CoreError::io(&exe, e))?;
    if !status.success() {
        return Err(CoreError::io(
            &exe,
            io::Error::other(format!(
                "ssh-keygen exited with code {}",
                status
                    .code()
                    .map_or_else(|| "unknown".to_string(), |c| c.to_string())
            )),
        ));
    }
    if !key.is_file() {
        return Err(CoreError::io(
            key,
            io::Error::new(io::ErrorKind::NotFound, "ssh-keygen didn't create the key"),
        ));
    }
    restrict_key_acl(key)
}

/// The account to grant in `icacls`: the current user's SID as `*S-1-…` (from
/// `whoami /user /fo csv /nh`, locale-independent and right for domain/Azure AD
/// accounts), else `USERDOMAIN\USERNAME`, else `USERNAME`.
#[cfg(windows)]
fn current_user_principal() -> Option<String> {
    let sid = locate_system_tool("whoami").ok().and_then(|exe| {
        let mut cmd = Command::new(exe);
        cmd.args(["/user", "/fo", "csv", "/nh"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        no_window(&mut cmd);
        let out = cmd.output().ok()?;
        out.status.success().then_some(())?;
        parse_whoami_sid(&String::from_utf8_lossy(&out.stdout))
    });
    if let Some(sid) = sid {
        return Some(format!("*{sid}"));
    }
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    match (var("USERDOMAIN"), var("USERNAME")) {
        (Some(d), Some(u)) => Some(format!("{d}\\{u}")),
        (None, Some(u)) => Some(u),
        _ => None,
    }
}

/// The SID field of `whoami /user /fo csv /nh` output
/// (`"domain\user","S-1-5-21-…"`).
pub fn parse_whoami_sid(stdout: &str) -> Option<String> {
    stdout
        .split([',', '\n', '\r'])
        .map(|f| f.trim().trim_matches('"'))
        .find(|f| {
            f.strip_prefix("S-1-").is_some_and(|rest| {
                !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            })
        })
        .map(str::to_string)
}

/// Locate a Windows system tool: `%SystemRoot%\System32` first (never a copy
/// planted on PATH), then PATH.
#[cfg(windows)]
fn locate_system_tool(name: &str) -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("SystemRoot") {
        let p = PathBuf::from(root)
            .join("System32")
            .join(format!("{name}.exe"));
        if p.is_file() {
            return Ok(p);
        }
    }
    locate_tool(name)
}

/// Restrict the private key to the current user (Windows: `icacls` with arg array —
/// `/inheritance:r /grant:r *<SID>:F` (fallback `DOMAIN\user`); checks exit code
/// only, output is locale-dependent. Unix: chmod 600).
pub fn restrict_key_acl(key: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let user = current_user_principal().ok_or_else(|| {
            CoreError::io(
                key,
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "couldn't find the current Windows user",
                ),
            )
        })?;
        let exe = locate_system_tool("icacls")?;
        let mut cmd = Command::new(&exe);
        cmd.arg(key)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{user}:F"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        no_window(&mut cmd);
        let status = cmd.status().map_err(|e| CoreError::io(&exe, e))?;
        if !status.success() {
            return Err(CoreError::io(
                key,
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "couldn't restrict the key to {user} (icacls exit {})",
                        status
                            .code()
                            .map_or_else(|| "unknown".to_string(), |c| c.to_string())
                    ),
                ),
            ));
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| CoreError::io(key, e))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = key;
        Ok(())
    }
}

/// Contents of `<key>.pub`, trimmed.
pub fn read_public_key(key: &Path) -> Result<String> {
    let path = pub_path(key);
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .map_err(|e| CoreError::io(path, e))
}

/// Parse ssh's stderr from `ssh -T git@github.com` into a [`ConnectionTest`].
pub fn parse_connection_output(stderr: &str, exit_code: Option<i32>) -> ConnectionTest {
    // GitHub greets with "Hi <user>! You've successfully authenticated, …" and exits 1.
    for line in stderr.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Hi ") {
            if let Some(end) = rest.find('!') {
                let user = rest[..end].trim();
                if !user.is_empty() && !user.contains(char::is_whitespace) {
                    return ConnectionTest::Connected {
                        user: user.to_string(),
                    };
                }
            }
        }
    }
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("permission denied (") && lower.contains("publickey") {
        return ConnectionTest::Denied;
    }
    const REASONS: &[(&str, &str)] = &[
        (
            "could not resolve hostname",
            "couldn't reach github.com (offline or DNS problem)",
        ),
        (
            "no such host is known",
            "couldn't reach github.com (offline or DNS problem)",
        ),
        ("timed out", "connection timed out"),
        ("network is unreachable", "network is unreachable"),
        ("connection refused", "connection refused"),
        ("connection reset", "connection was reset"),
        ("connection closed", "connection was closed by the server"),
        (
            "host key verification failed",
            "host key verification failed",
        ),
        (
            "remote host identification has changed",
            "github.com host key changed (check ~/.ssh/known_hosts)",
        ),
        (
            "unprotected private key file",
            "the key file can be read by other users",
        ),
        ("bad permissions", "the key file can be read by other users"),
        ("incorrect passphrase", "wrong passphrase"),
        ("no such file or directory", "key file not found"),
        ("not accessible", "key file not found"),
        ("invalid format", "the key file isn't a valid private key"),
    ];
    for (needle, reason) in REASONS {
        if lower.contains(needle) {
            return ConnectionTest::Failed {
                reason: (*reason).to_string(),
            };
        }
    }
    let last = stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("Warning: Permanently added"));
    let reason = match (last, exit_code) {
        (Some(l), _) => truncate(l, 120),
        (None, Some(code)) => format!("ssh exited with code {code}"),
        (None, None) => "ssh stopped without an answer".to_string(),
    };
    ConnectionTest::Failed { reason }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// **Network (user-initiated only).** `ssh -T -i <key> -o IdentitiesOnly=yes
/// -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new git@github.com`.
///
/// `BatchMode` is not forced, so ssh can ask for the key passphrase on the console
/// (GitHop never sees it). ssh missing → `Ok(Failed)`.
pub fn test_connection(key: &Path) -> Result<ConnectionTest> {
    // The same ssh a pinned repo uses (Windows OpenSSH when present), so the
    // test sees the same agent and known_hosts as a real push.
    let exe = match git_ssh_program().map_or_else(|| locate_tool("ssh"), Ok) {
        Ok(p) => p,
        Err(_) => {
            return Ok(ConnectionTest::Failed {
                reason: "ssh was not found. Install Git for Windows or OpenSSH".to_string(),
            })
        }
    };
    let out = Command::new(&exe)
        .arg("-T")
        .arg("-i")
        .arg(ssh_identity_arg(key))
        .args([
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "git@github.com",
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| CoreError::io(&exe, e))?;
    let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.stdout.is_empty() {
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&out.stdout));
    }
    Ok(parse_connection_output(&text, out.status.code()))
}

/// The `-i` argument for `key`: ssh expands `%` tokens in identity paths, so a
/// literal `%` is passed as `%%` (non-UTF-8 paths are passed unchanged).
fn ssh_identity_arg(key: &Path) -> OsString {
    match key.to_str() {
        Some(s) if s.contains('%') => OsString::from(crate::pin::escape_ssh_percent(s)),
        _ => key.as_os_str().to_owned(),
    }
}

/// `ssh -G <alias>` → the `hostname` value (lower-cased). Too slow for hooks; scan/doctor only.
pub fn resolve_alias_hostname(alias: &str) -> Option<String> {
    let alias = alias.trim();
    // Never let an alias be read as an option.
    if alias.is_empty() || alias.starts_with('-') || alias.chars().any(|c| c.is_control()) {
        return None;
    }
    let exe = locate_tool("ssh").ok()?;
    let mut cmd = Command::new(&exe);
    cmd.arg("-G")
        .arg("--")
        .arg(alias)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    no_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ssh_g_hostname(&String::from_utf8_lossy(&out.stdout))
}

/// The `hostname <value>` line from `ssh -G` output, lower-cased.
fn parse_ssh_g_hostname(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let (k, v) = line.trim().split_once(char::is_whitespace)?;
        let v = v.trim();
        (k.eq_ignore_ascii_case("hostname") && !v.is_empty()).then(|| v.to_ascii_lowercase())
    })
}

/// Windows: query the `ssh-agent` service (`sc query ssh-agent`, exit code / STATE parse);
/// other OSes: `Unknown`.
pub fn agent_status() -> AgentStatus {
    #[cfg(windows)]
    {
        let Ok(exe) = locate_system_tool("sc") else {
            return AgentStatus::Unknown;
        };
        let mut cmd = Command::new(&exe);
        cmd.args(["query", "ssh-agent"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        no_window(&mut cmd);
        match cmd.output() {
            Ok(out) if out.status.success() => {
                parse_sc_query(&String::from_utf8_lossy(&out.stdout))
            }
            _ => AgentStatus::Unknown,
        }
    }
    #[cfg(not(windows))]
    {
        AgentStatus::Unknown
    }
}

/// Parse `sc query` output. Prefers the numeric state code (locale-independent:
/// `1` stopped, `4` running) on the `STATE` line, then the `RUNNING`/`STOPPED` words
/// anywhere; anything else → `Unknown`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_sc_query(stdout: &str) -> AgentStatus {
    let from_words = |s: &str| {
        let words: Vec<&str> = s
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .collect();
        if words.contains(&"RUNNING") {
            Some(AgentStatus::Running)
        } else if words.contains(&"STOPPED") {
            Some(AgentStatus::Stopped)
        } else {
            None
        }
    };
    for line in stdout.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        if label.trim().eq_ignore_ascii_case("state") {
            match value.split_whitespace().next() {
                Some("4") => return AgentStatus::Running,
                Some("1") => return AgentStatus::Stopped,
                Some(_) => return from_words(value).unwrap_or(AgentStatus::Unknown),
                None => {}
            }
        }
    }
    stdout
        .lines()
        .find_map(from_words)
        .unwrap_or(AgentStatus::Unknown)
}

// ---------------------------------------------------------------------------
// GitHub host keys (ADR 0006)

/// GitHub's published SSH host keys, as `known_hosts` lines.
///
/// Source: GitHub Docs, "GitHub's SSH key fingerprints"
/// <https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints>.
/// The RSA key is the one GitHub rotated to on 2023-03-24. A unit test checks
/// each line against [`GITHUB_FINGERPRINTS`] (SHA-256, as published).
pub const GITHUB_KNOWN_HOSTS: [&str; 3] = [
    "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
    "github.com ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=",
    "github.com ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=",
];

/// SHA-256 fingerprints GitHub publishes for [`GITHUB_KNOWN_HOSTS`] (same order).
pub const GITHUB_FINGERPRINTS: [&str; 3] = [
    "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU",
    "SHA256:p2QAMXNIC1TJYWeIOttrVc98/R1BUFWu3/LiyKgUfQM",
    "SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s",
];

/// What [`ensure_github_known_hosts`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum KnownHostsOutcome {
    /// `known_hosts` already trusts one of GitHub's current keys.
    AlreadyTrusted,
    /// GitHub's keys were added. `backup` = copy of the previous file, if there was one.
    Added { backup: Option<PathBuf> },
}

/// `~/.ssh/known_hosts` under the home dir git/ssh would use.
pub fn known_hosts_path(git: &Git) -> Option<PathBuf> {
    ssh_dir(git).map(|d| d.join("known_hosts"))
}

/// The base64 key blob of each current GitHub key.
fn github_blobs() -> impl Iterator<Item = &'static str> {
    GITHUB_KNOWN_HOSTS
        .iter()
        .filter_map(|l| l.split_whitespace().nth(2))
}

/// Does a host pattern list (`github.com,140.82.112.3`, `[github.com]:22`,
/// `gh*`) name `github.com`? Hashed (`|1|…`) and negated patterns don't count.
fn hosts_field_names_github(field: &str) -> bool {
    let mut positive = false;
    for pat in field.split(',') {
        let pat = pat.trim();
        if pat.is_empty() || pat.starts_with('|') {
            continue;
        }
        let (neg, pat) = match pat.strip_prefix('!') {
            Some(p) => (true, p),
            None => (false, pat),
        };
        let host = match pat.strip_prefix('[') {
            Some(rest) => match rest.split_once("]:") {
                Some((h, "22")) => h,
                _ => continue,
            },
            None => pat,
        };
        if glob_match(&host.to_ascii_lowercase(), GITHUB_HOST) {
            if neg {
                return false;
            }
            positive = true;
        }
    }
    positive
}

/// True when `known_hosts` text already trusts one of GitHub's **current**
/// keys for `github.com` (a line with only GitHub's old, rotated RSA key
/// doesn't count). `@revoked` lines never count. Pure.
pub fn known_hosts_trusts_github(text: &str) -> bool {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return false;
        }
        let mut words = line.split_whitespace();
        let mut first = words.next().unwrap_or_default();
        if first.starts_with('@') {
            if first.eq_ignore_ascii_case("@revoked") {
                return false;
            }
            first = words.next().unwrap_or_default();
        }
        let _key_type = words.next();
        let blob = words.next().unwrap_or_default();
        hosts_field_names_github(first) && github_blobs().any(|b| b == blob)
    })
}

/// Add GitHub's published host keys to `~/.ssh/known_hosts` unless it already
/// trusts one of them, so the first SSH connect (terminal, VS Code, the app)
/// never stops at "Are you sure you want to continue connecting (yes/no)?".
///
/// Creates `~/.ssh` and the file when missing; keeps every existing line; copies
/// the old file to GitHop's backups folder first and writes atomically (temp
/// file + rename, CLAUDE.md rule 4). No network: the keys are compiled in.
pub fn ensure_github_known_hosts(git: &Git, paths: &crate::Paths) -> Result<KnownHostsOutcome> {
    let path = known_hosts_path(git).ok_or(CoreError::NoDataDir)?;
    let existing = match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(CoreError::io(&path, e)),
    };
    let text = existing
        .as_deref()
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    if known_hosts_trusts_github(&text) {
        return Ok(KnownHostsOutcome::AlreadyTrusted);
    }
    let backup = match &existing {
        Some(bytes) => {
            let dir = paths.backups_dir();
            std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
            let to = dir.join(format!("known_hosts.{}", crate::backup::timestamp()));
            std::fs::write(&to, bytes).map_err(|e| CoreError::io(&to, e))?;
            Some(to)
        }
        None => None,
    };
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .ok_or(CoreError::NoDataDir)?;
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out: Vec<u8> = existing.unwrap_or_default();
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.extend_from_slice(newline.as_bytes());
    }
    out.extend_from_slice(format!("# GitHub host keys, added by GitHop{newline}").as_bytes());
    for line in GITHUB_KNOWN_HOSTS {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(newline.as_bytes());
    }
    let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| CoreError::io(&dir, e))?;
    io::Write::write_all(&mut tmp, &out).map_err(|e| CoreError::io(tmp.path(), e))?;
    tmp.persist(&path)
        .map_err(|e| CoreError::io(&path, e.error))?;
    Ok(KnownHostsOutcome::Added { backup })
}

/// [`ensure_github_known_hosts`], ignoring errors: the caller's real work
/// (pin, keygen, test) must never fail over it.
pub fn ensure_github_known_hosts_best_effort(git: &Git, paths: &crate::Paths) {
    let _ = ensure_github_known_hosts(git, paths);
}

// ---------------------------------------------------------------------------
// Which ssh git runs, and the Windows ssh-agent (ADR 0006)

/// `%SystemRoot%\System32\OpenSSH\<name>.exe` when it exists (Windows' own
/// OpenSSH, the one that talks to the Windows `ssh-agent` service).
pub fn windows_openssh_tool(name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot")?;
        let p = PathBuf::from(root)
            .join("System32")
            .join("OpenSSH")
            .join(format!("{name}.exe"));
        p.is_file().then_some(p)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        None
    }
}

/// The ssh program GitHop writes into `core.sshCommand`: Windows' OpenSSH
/// (`C:\Windows\System32\OpenSSH\ssh.exe`) when it exists, else `None` (plain
/// `ssh`). Git for Windows bundles its own ssh, which can't use the Windows
/// ssh-agent, so a passphrase would be asked on every push — and VS Code's
/// Git, which has no terminal, can't ask at all.
pub fn git_ssh_program() -> Option<PathBuf> {
    windows_openssh_tool("ssh")
}

/// Is the passphrase of the Windows ssh-agent's key remembered, and can it be?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PassphraseState {
    /// The key file doesn't exist.
    NoKey,
    /// The key has no passphrase: nothing to remember.
    NoPassphrase,
    /// The Windows ssh-agent holds this key: no prompt on push.
    Remembered,
    /// The agent runs but doesn't hold the key: ssh asks on each push.
    AsksEachTime,
    /// The Windows ssh-agent service is stopped or disabled.
    AgentOff,
    /// Not Windows, or GitHop can't tell.
    Unknown,
}

/// Result of [`remember_passphrase`] / [`turn_on_agent_and_remember`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RememberOutcome {
    /// The agent now holds the key.
    Remembered,
    /// The key has no passphrase.
    NoPassphrase,
    /// The ssh-agent service isn't running (turn it on first).
    AgentOff,
    /// The Windows admin (UAC) prompt was declined.
    Cancelled,
    /// `ssh-add` didn't add the key (console closed, wrong passphrase).
    NotAdded,
}

/// Everything the passphrase flow does to the system, so tests can stub it
/// (they never start an elevated process or a real `ssh-add`).
pub trait AgentHost {
    /// State of the Windows `ssh-agent` service.
    fn service(&self) -> AgentStatus;
    /// SHA-256 fingerprints the agent holds (`ssh-add -l`); `None` = can't ask.
    fn fingerprints(&self) -> Option<Vec<String>>;
    /// `ssh-add <key>` in a visible console; the user types the passphrase
    /// there. `Ok(true)` when ssh-add exited 0.
    fn add_key_in_console(&self, key: &Path) -> Result<bool>;
    /// Elevated (UAC): set the service to start automatically and start it.
    /// `Ok(false)` when the prompt was declined.
    fn turn_on_service(&self) -> Result<bool>;
    /// Send one agent-protocol request (an unlocked key, ADR 0007) to the
    /// Windows ssh-agent over its local pipe. `Ok(true)` when it accepted.
    fn send_agent_request(&self, request: &[u8]) -> Result<bool>;
}

/// `SHA256:<base64, no padding>` of a public key line (`type blob [comment]`). Pure.
pub fn public_key_fingerprint(line: &str) -> Option<String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let blob = line.split_whitespace().nth(1)?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(blob)
        .ok()?;
    let digest = sha2::Sha256::digest(&raw);
    Some(format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    ))
}

/// Fingerprints from `ssh-add -l` output (`256 SHA256:… comment (ED25519)`). Pure.
pub fn parse_ssh_add_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .find(|w| w.starts_with("SHA256:"))
                .map(str::to_string)
        })
        .collect()
}

/// Is this private key file protected by a passphrase? Reads only the header
/// (OpenSSH format: the cipher name; PEM: `ENCRYPTED`). `None` for a format
/// GitHop doesn't know. Pure; the passphrase itself is never involved.
pub fn private_key_is_encrypted(text: &str) -> Option<bool> {
    use base64::Engine as _;
    const OPENSSH: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
    if let Some(start) = text.find(OPENSSH) {
        let body = &text[start + OPENSSH.len()..];
        let end = body.find("-----END")?;
        let b64: zeroize::Zeroizing<String> =
            zeroize::Zeroizing::new(body[..end].chars().filter(|c| !c.is_whitespace()).collect());
        let raw = zeroize::Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(b64.as_bytes())
                .ok()?,
        );
        let rest = raw.strip_prefix(b"openssh-key-v1\0")?;
        let len = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let cipher = rest.get(4..4 + len)?;
        return Some(cipher != b"none");
    }
    if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        return Some(true);
    }
    if text.contains("-----BEGIN PRIVATE KEY-----") {
        return Some(false);
    }
    if text.contains(" PRIVATE KEY-----") {
        return Some(text.contains("ENCRYPTED"));
    }
    None
}

/// Read the key file's header (the content is wiped from memory afterwards).
fn key_encrypted(key: &Path) -> Option<bool> {
    let bytes = zeroize::Zeroizing::new(std::fs::read(key).ok()?);
    let text = zeroize::Zeroizing::new(String::from_utf8_lossy(&bytes).into_owned());
    private_key_is_encrypted(&text)
}

/// Fingerprint of `<key>.pub`, if it exists.
fn key_fingerprint(key: &Path) -> Option<String> {
    read_public_key(key)
        .ok()
        .and_then(|p| public_key_fingerprint(&p))
}

/// Status line for the key panel.
pub fn passphrase_state(host: &dyn AgentHost, key: &Path) -> PassphraseState {
    if key.as_os_str().is_empty() || !key.is_file() {
        return PassphraseState::NoKey;
    }
    if key_encrypted(key) == Some(false) {
        return PassphraseState::NoPassphrase;
    }
    match host.service() {
        AgentStatus::Running => {
            let (Some(fp), Some(held)) = (key_fingerprint(key), host.fingerprints()) else {
                return PassphraseState::Unknown;
            };
            if held.contains(&fp) {
                PassphraseState::Remembered
            } else {
                PassphraseState::AsksEachTime
            }
        }
        AgentStatus::Stopped => PassphraseState::AgentOff,
        AgentStatus::Unknown => PassphraseState::Unknown,
    }
}

/// "Remember my passphrase": with the agent running, run `ssh-add <key>` in a
/// visible console (the passphrase is typed there, never through GitHop).
pub fn remember_passphrase(host: &dyn AgentHost, key: &Path) -> Result<RememberOutcome> {
    if !key.is_file() {
        return Err(CoreError::io(
            key,
            io::Error::new(io::ErrorKind::NotFound, "the key file doesn't exist"),
        ));
    }
    if key_encrypted(key) == Some(false) {
        return Ok(RememberOutcome::NoPassphrase);
    }
    if host.service() != AgentStatus::Running {
        return Ok(RememberOutcome::AgentOff);
    }
    let added = host.add_key_in_console(key)?;
    let held = match (key_fingerprint(key), host.fingerprints()) {
        (Some(fp), Some(list)) => list.contains(&fp),
        _ => added,
    };
    Ok(if held {
        RememberOutcome::Remembered
    } else {
        RememberOutcome::NotAdded
    })
}

/// "Turn on": one UAC prompt sets the ssh-agent service to start automatically
/// and starts it, then [`remember_passphrase`].
pub fn turn_on_agent_and_remember(host: &dyn AgentHost, key: &Path) -> Result<RememberOutcome> {
    if key.is_file() && key_encrypted(key) == Some(false) {
        return Ok(RememberOutcome::NoPassphrase);
    }
    if let Some(stopped) = ensure_agent_running(host)? {
        return Ok(stopped);
    }
    remember_passphrase(host, key)
}

/// Start the agent if needed (one UAC prompt). `Some(outcome)` when it isn't
/// running afterwards (prompt declined, or the service never came up).
fn ensure_agent_running(host: &dyn AgentHost) -> Result<Option<RememberOutcome>> {
    if host.service() == AgentStatus::Running {
        return Ok(None);
    }
    if !host.turn_on_service()? {
        return Ok(Some(RememberOutcome::Cancelled));
    }
    // The service can take a moment to report RUNNING.
    for _ in 0..20 {
        if host.service() == AgentStatus::Running {
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok(Some(RememberOutcome::AgentOff))
}

/// In-app "Remember my passphrase" (ADR 0007): unlock `key` with the passphrase
/// typed in GitHop and hand it to the Windows ssh-agent directly. The
/// passphrase lives only in memory for this call. With `turn_on`, a stopped
/// agent is started first (one UAC prompt); otherwise it reports `AgentOff`.
///
/// [`CoreError::WrongPassphrase`] when the passphrase doesn't match;
/// [`CoreError::Key`] for a key type that needs the console flow instead.
pub fn remember_with_passphrase(
    host: &dyn AgentHost,
    key: &Path,
    passphrase: Option<&str>,
    turn_on: bool,
) -> Result<RememberOutcome> {
    if !key.is_file() {
        return Err(CoreError::io(
            key,
            io::Error::new(io::ErrorKind::NotFound, "the key file doesn't exist"),
        ));
    }
    if key_encrypted(key) == Some(false) {
        return Ok(RememberOutcome::NoPassphrase);
    }
    // Check the passphrase before any UAC prompt, so a typo never costs one.
    let unlocked = crate::keyfile::unlock_ed25519(key, passphrase)?;
    if turn_on {
        if let Some(stopped) = ensure_agent_running(host)? {
            return Ok(stopped);
        }
    } else if host.service() != AgentStatus::Running {
        return Ok(RememberOutcome::AgentOff);
    }
    let accepted = host.send_agent_request(&crate::keyfile::agent_add_request(&unlocked))?;
    let held = match (key_fingerprint(key), host.fingerprints()) {
        (Some(fp), Some(list)) => list.contains(&fp),
        _ => accepted,
    };
    Ok(if held {
        RememberOutcome::Remembered
    } else {
        RememberOutcome::NotAdded
    })
}

/// The real Windows ssh-agent (`sc`, Windows OpenSSH's `ssh-add`, UAC).
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsAgent;

/// PowerShell run (not elevated) to start ONE elevated `cmd.exe` through
/// `Start-Process -Verb RunAs` (ShellExecute "runas": the UAC prompt), which
/// runs `sc.exe config ssh-agent start= auto` and `sc.exe start ssh-agent`. A
/// compile-time constant: no user input is ever part of it. Exit 1223 = the
/// prompt was declined.
///
/// Both exe paths (the elevated `cmd.exe` and the `sc.exe` it runs) are absolute
/// paths from `[Environment]::SystemDirectory` (GetSystemDirectory), never from
/// an environment variable. Another program running as the user could change
/// `%SystemRoot%` or `%windir%`, including in `HKCU\Environment`, which the
/// elevated process reads too, and have its own exe run elevated through
/// GitHop's prompt. `/d` skips cmd's AutoRun commands for the same reason.
pub const ELEVATE_AGENT_SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
try { \
$sys = [Environment]::SystemDirectory; \
$sc = Join-Path $sys 'sc.exe'; \
$p = Start-Process -FilePath (Join-Path $sys 'cmd.exe') \
-ArgumentList '/d', '/c', ('{0} config ssh-agent start= auto & {0} start ssh-agent' -f $sc) \
-Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode \
} catch { exit 1223 }";

/// Exit code of [`ELEVATE_AGENT_SCRIPT`] when the UAC prompt was declined.
pub const ELEVATION_DECLINED: i32 = 1223;

impl AgentHost for WindowsAgent {
    fn service(&self) -> AgentStatus {
        agent_status()
    }

    fn fingerprints(&self) -> Option<Vec<String>> {
        let exe = windows_openssh_tool("ssh-add")?;
        let mut cmd = Command::new(exe);
        cmd.arg("-l")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        no_window(&mut cmd);
        let out = cmd.output().ok()?;
        match out.status.code() {
            // 1 = "The agent has no identities."
            Some(0) | Some(1) => Some(parse_ssh_add_list(&String::from_utf8_lossy(&out.stdout))),
            _ => None,
        }
    }

    fn add_key_in_console(&self, key: &Path) -> Result<bool> {
        let exe = windows_openssh_tool("ssh-add").ok_or_else(|| {
            CoreError::io(
                "ssh-add",
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "Windows OpenSSH (ssh-add) isn't installed",
                ),
            )
        })?;
        let mut cmd = Command::new(&exe);
        cmd.arg("--").arg(key);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            cmd.creation_flags(CREATE_NEW_CONSOLE);
        }
        let status = cmd.status().map_err(|e| CoreError::io(&exe, e))?;
        Ok(status.success())
    }

    fn send_agent_request(&self, request: &[u8]) -> Result<bool> {
        crate::keyfile::agent_send(request)
    }

    fn turn_on_service(&self) -> Result<bool> {
        #[cfg(windows)]
        {
            let root = std::env::var_os("SystemRoot").ok_or(CoreError::NoDataDir)?;
            let exe = PathBuf::from(root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            let mut cmd = Command::new(&exe);
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                ELEVATE_AGENT_SCRIPT,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
            no_window(&mut cmd);
            let status = cmd.status().map_err(|e| CoreError::io(&exe, e))?;
            Ok(status.code() != Some(ELEVATION_DECLINED))
        }
        #[cfg(not(windows))]
        {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Found = Vec<(String, String, usize)>;

    fn conflicts(cfg: &str) -> Found {
        find_github_conflicts(cfg)
            .into_iter()
            .map(|c| (c.host_pattern, c.identity_file, c.line))
            .collect()
    }

    fn one(p: &str, f: &str, l: usize) -> Found {
        vec![(p.to_string(), f.to_string(), l)]
    }

    #[test]
    fn ssh_config_table() {
        let cases: &[(&str, &str, Found)] = &[
            ("empty", "", vec![]),
            (
                "exact host",
                "Host github.com\n  IdentityFile ~/.ssh/id_work\n",
                one("github.com", "~/.ssh/id_work", 2),
            ),
            (
                "wildcard star",
                "Host *\n    IdentityFile ~/.ssh/id_rsa\n",
                one("*", "~/.ssh/id_rsa", 2),
            ),
            (
                "star dot com",
                "Host *.com\nIdentityFile k\n",
                one("*.com", "k", 2),
            ),
            (
                "github prefix glob",
                "Host github*\nIdentityFile k\n",
                one("github*", "k", 2),
            ),
            (
                "question mark glob",
                "Host github.co?\nIdentityFile k\n",
                one("github.co?", "k", 2),
            ),
            (
                "multiple hosts per line",
                "Host gitlab.com github.com bitbucket.org\nIdentityFile k\n",
                one("gitlab.com github.com bitbucket.org", "k", 2),
            ),
            (
                "comma list",
                "Host gitlab.com,github.com\nIdentityFile k\n",
                one("gitlab.com,github.com", "k", 2),
            ),
            (
                "other host only",
                "Host gitlab.com\nIdentityFile k\n",
                vec![],
            ),
            (
                "negation excludes",
                "Host * !github.com\nIdentityFile k\n",
                vec![],
            ),
            (
                "negation of other host still matches",
                "Host * !gitlab.com\nIdentityFile k\n",
                one("* !gitlab.com", "k", 2),
            ),
            (
                "only negated never matches",
                "Host !gitlab.com\nIdentityFile k\n",
                vec![],
            ),
            (
                "match ends host block",
                "Host github.com\nUser git\nMatch host github.com\nIdentityFile k\n",
                vec![],
            ),
            (
                "host after match restarts",
                "Match all\nIdentityFile a\nHost github.com\nIdentityFile b\n",
                one("github.com", "b", 4),
            ),
            (
                "equals form",
                "Host=github.com\nIdentityFile=~/.ssh/k\n",
                one("github.com", "~/.ssh/k", 2),
            ),
            (
                "equals with spaces",
                "Host = github.com\n  IdentityFile = ~/.ssh/k\n",
                one("github.com", "~/.ssh/k", 2),
            ),
            (
                "quoted value with spaces",
                "Host \"github.com\"\nIdentityFile \"C:/Users/My Name/.ssh/k\"\n",
                one("github.com", "C:/Users/My Name/.ssh/k", 2),
            ),
            (
                "case-insensitive keywords and host",
                "HOST GitHub.COM\nidentityfile k\n",
                one("GitHub.COM", "k", 2),
            ),
            (
                "comments ignored",
                "# Host github.com\n#IdentityFile x\nHost github.com # work\n  # IdentityFile y\n  IdentityFile k # main key\n",
                one("github.com", "k", 5),
            ),
            (
                "crlf line endings",
                "Host github.com\r\n  IdentityFile k\r\n",
                one("github.com", "k", 2),
            ),
            (
                "no identity file",
                "Host github.com\n  User git\n  IdentitiesOnly yes\n",
                vec![],
            ),
            (
                "alias with hostname is not a conflict",
                "Host github-personal\n  HostName github.com\n  IdentityFile ~/.ssh/personal\n",
                vec![],
            ),
            (
                "global section before any host",
                "IdentityFile ~/.ssh/id_all\nHost gitlab.com\nIdentityFile g\n",
                one("*", "~/.ssh/id_all", 1),
            ),
            (
                "include not followed",
                "Include ~/.ssh/github.conf\nHost gitlab.com\n",
                vec![],
            ),
            (
                "tabs and blank lines",
                "\n\tHost\tgithub.com\n\n\tIdentityFile\t~/.ssh/k\n",
                one("github.com", "~/.ssh/k", 4),
            ),
            (
                "bom",
                "\u{feff}Host github.com\nIdentityFile k\n",
                one("github.com", "k", 2),
            ),
            (
                "keyword with empty value is ignored",
                "Host github.com\nIdentityFile\n",
                vec![],
            ),
        ];
        for (name, cfg, want) in cases {
            assert_eq!(&conflicts(cfg), want, "case: {name}");
        }
    }

    #[test]
    fn multiple_identity_files_and_blocks() {
        let cfg = "Host github.com\n IdentityFile a\n IdentityFile b\nHost *\n IdentityFile c\n";
        let got = conflicts(cfg);
        assert_eq!(
            got,
            vec![
                ("github.com".into(), "a".into(), 2),
                ("github.com".into(), "b".into(), 3),
                ("*".into(), "c".into(), 5),
            ]
        );
    }

    #[test]
    fn parser_never_panics_on_garbage() {
        for cfg in [
            "=",
            "==\n=",
            "\"",
            "Host \"",
            "Host\n\"IdentityFile",
            "#",
            "Host * *** ?*?",
        ] {
            let _ = find_github_conflicts(cfg);
        }
    }

    #[test]
    fn glob_cases() {
        assert!(glob_match("*", "github.com"));
        assert!(glob_match("g*b.c*m", "github.com"));
        assert!(glob_match("**", ""));
        assert!(!glob_match("github.com?", "github.com"));
        assert!(!glob_match("*.org", "github.com"));
    }

    #[test]
    fn connection_output_table() {
        let cases: &[(&str, Option<i32>, ConnectionTest)] = &[
            (
                "Hi jane-personal! You've successfully authenticated, but GitHub does not provide shell access.\n",
                Some(1),
                ConnectionTest::Connected { user: "jane-personal".into() },
            ),
            (
                "Warning: Permanently added 'github.com' (ED25519) to the list of known hosts.\r\nHi octo-org! You've successfully authenticated, but GitHub does not provide shell access.\r\n",
                Some(1),
                ConnectionTest::Connected { user: "octo-org".into() },
            ),
            ("git@github.com: Permission denied (publickey).\n", Some(255), ConnectionTest::Denied),
            (
                "git@github.com: Permission denied (publickey,keyboard-interactive).",
                Some(255),
                ConnectionTest::Denied,
            ),
            (
                "ssh: connect to host github.com port 22: Connection timed out\n",
                Some(255),
                ConnectionTest::Failed { reason: "connection timed out".into() },
            ),
            (
                "ssh: Could not resolve hostname github.com: No such host is known.\n",
                Some(255),
                ConnectionTest::Failed {
                    reason: "couldn't reach github.com (offline or DNS problem)".into(),
                },
            ),
            (
                "ssh: connect to host github.com port 22: Connection refused",
                Some(255),
                ConnectionTest::Failed { reason: "connection refused".into() },
            ),
            (
                "Host key verification failed.",
                Some(255),
                ConnectionTest::Failed { reason: "host key verification failed".into() },
            ),
            (
                "@@@@\n@         WARNING: UNPROTECTED PRIVATE KEY FILE!          @\n",
                Some(255),
                ConnectionTest::Failed { reason: "the key file can be read by other users".into() },
            ),
            (
                "Warning: Identity file C:/x/k not accessible: No such file or directory.\ngit@github.com: Permission denied (publickey).",
                Some(255),
                ConnectionTest::Denied,
            ),
            (
                "kex_exchange_identification: something odd happened",
                Some(255),
                ConnectionTest::Failed { reason: "kex_exchange_identification: something odd happened".into() },
            ),
            ("", Some(255), ConnectionTest::Failed { reason: "ssh exited with code 255".into() }),
            ("", None, ConnectionTest::Failed { reason: "ssh stopped without an answer".into() }),
            (
                "Hi ! weird",
                Some(1),
                ConnectionTest::Failed { reason: "Hi ! weird".into() },
            ),
        ];
        for (stderr, code, want) in cases {
            assert_eq!(
                &parse_connection_output(stderr, *code),
                want,
                "stderr: {stderr:?}"
            );
        }
    }

    #[test]
    fn long_unknown_reason_is_truncated() {
        let long = "x".repeat(300);
        match parse_connection_output(&long, Some(255)) {
            ConnectionTest::Failed { reason } => assert_eq!(reason.chars().count(), 121),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ssh_g_hostname() {
        let out = "user git\nhostname GitHub.com\nport 22\n";
        assert_eq!(parse_ssh_g_hostname(out).as_deref(), Some("github.com"));
        assert_eq!(parse_ssh_g_hostname("user git\n"), None);
        assert_eq!(parse_ssh_g_hostname("hostnamex y\n"), None);
    }

    #[test]
    fn resolve_alias_rejects_options() {
        assert_eq!(resolve_alias_hostname("-oProxyCommand=calc"), None);
        assert_eq!(resolve_alias_hostname(""), None);
    }

    #[test]
    fn sc_query_parse() {
        let running = "\r\nSERVICE_NAME: ssh-agent \r\n        TYPE               : 10  WIN32_OWN_PROCESS  \r\n        STATE              : 4  RUNNING \r\n                                (STOPPABLE, NOT_PAUSABLE, IGNORES_SHUTDOWN)\r\n";
        assert_eq!(parse_sc_query(running), AgentStatus::Running);
        let stopped = "SERVICE_NAME: ssh-agent\n        STATE              : 1  STOPPED\n";
        assert_eq!(parse_sc_query(stopped), AgentStatus::Stopped);
        // Localised label: falls back to the state word.
        let localised = "SERVICE_NAME: ssh-agent\n        ZUSTAND            : 1  STOPPED\n";
        assert_eq!(parse_sc_query(localised), AgentStatus::Stopped);
        let pending = "        STATE              : 2  START_PENDING\n";
        assert_eq!(parse_sc_query(pending), AgentStatus::Unknown);
        assert_eq!(parse_sc_query("garbage"), AgentStatus::Unknown);
    }

    #[test]
    fn whoami_sid_parse() {
        assert_eq!(
            parse_whoami_sid("\"desktop-x\\jane\",\"S-1-5-21-390-301-410-1001\"\r\n").as_deref(),
            Some("S-1-5-21-390-301-410-1001")
        );
        assert_eq!(
            parse_whoami_sid("\"AzureAD\\Jane Doe\",\"S-1-12-1-111-222\"").as_deref(),
            Some("S-1-12-1-111-222")
        );
        assert_eq!(parse_whoami_sid(""), None);
        assert_eq!(parse_whoami_sid("\"S-1-\",\"x\""), None);
        assert_eq!(parse_whoami_sid("\"S-1-5;evil\""), None);
    }

    #[test]
    fn identity_arg_escapes_percent() {
        assert_eq!(
            ssh_identity_arg(Path::new("C:/k/100%/id")),
            OsString::from("C:/k/100%%/id")
        );
        assert_eq!(
            ssh_identity_arg(Path::new("C:/k/id")),
            OsString::from("C:/k/id")
        );
    }

    /// Runs the real `whoami` + `icacls` on a temp file (no network, no real keys).
    #[cfg(windows)]
    #[test]
    fn restrict_key_acl_grants_only_the_current_user() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("githop test key");
        std::fs::write(&key, b"not a real key").unwrap();
        let principal = current_user_principal().unwrap();
        assert!(principal.starts_with("*S-1-"), "{principal}");
        restrict_key_acl(&key).unwrap();
        // Still readable by us.
        assert_eq!(std::fs::read(&key).unwrap(), b"not a real key");
        let out = Command::new(locate_system_tool("icacls").unwrap())
            .arg(&key)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        let user = std::env::var("USERNAME").unwrap().to_lowercase();
        let aces: Vec<&str> = text.lines().filter(|l| l.contains(":(")).collect();
        assert_eq!(aces.len(), 1, "{text}");
        assert!(aces[0].contains(&user) && aces[0].contains("(f)"), "{text}");
    }

    #[test]
    fn embedded_github_keys_match_published_fingerprints() {
        for (line, fp) in GITHUB_KNOWN_HOSTS.iter().zip(GITHUB_FINGERPRINTS) {
            let mut w = line.split_whitespace();
            assert_eq!(w.next(), Some("github.com"));
            let key = w.collect::<Vec<_>>().join(" ");
            assert_eq!(public_key_fingerprint(&key).as_deref(), Some(fp), "{line}");
        }
    }

    #[test]
    fn known_hosts_detection_table() {
        let ed = GITHUB_KNOWN_HOSTS[0];
        let ed_blob = ed.split_whitespace().nth(2).unwrap();
        let old_rsa = "github.com ssh-rsa AAAAB3NzaC1yc2EAAAABIwAAAQEAq2A7hRGmdnm9tUDbO9IDSwBK6TbQa+PXYPCPy6rbTrTtw7PHkccKrpp0yVhp5HdEIcKr6pLlVDBfOLX9QUsyCOV0wzfjIJNlGEYsdlLJizHhbn2mUjvSAHQqZETYP81eFzLQNnPHt4EVVUh7VfDESU84KezmD5QlWpXLmvU31/yMf+Se8xhHTvKSCZIFImWwoG6mbUoWf9nzpIoaSjB+weqqUUmpaaasXVal72J+UX2B+2RPW3RcT0eOzQgqlJL3RKrTJvdsjE3JEAvGq3lGHSZXy28G3skua2SmVi/w4yCE6gbODqnTWlg7+wC604ydGXA8VJiS5ap43JXiUFFAaQ==";
        let cases: &[(&str, &str, bool)] = &[
            ("empty", "", false),
            ("plain ed25519", ed, true),
            ("crlf + bom", &format!("\u{feff}{ed}\r\n"), true),
            (
                "ip list",
                &format!("github.com,140.82.112.3 ssh-ed25519 {ed_blob}"),
                true,
            ),
            (
                "port 22 bracket",
                &format!("[github.com]:22 ssh-ed25519 {ed_blob}"),
                true,
            ),
            (
                "port 443 bracket",
                &format!("[github.com]:443 ssh-ed25519 {ed_blob}"),
                false,
            ),
            (
                "other host",
                &format!("gitlab.com ssh-ed25519 {ed_blob}"),
                false,
            ),
            (
                "revoked",
                &format!("@revoked github.com ssh-ed25519 {ed_blob}"),
                false,
            ),
            (
                "hashed",
                &format!("|1|abc=|def= ssh-ed25519 {ed_blob}"),
                false,
            ),
            ("old rotated rsa only", old_rsa, false),
            ("comment", &format!("# {ed}"), false),
            (
                "negated",
                &format!("*,!github.com ssh-ed25519 {ed_blob}"),
                false,
            ),
            ("glob", &format!("*.com ssh-ed25519 {ed_blob}"), true),
            ("garbage", "github.com", false),
        ];
        for (name, text, want) in cases {
            assert_eq!(known_hosts_trusts_github(text), *want, "case {name}");
        }
    }

    fn known_hosts_env() -> (tempfile::TempDir, Git, crate::Paths) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let git = Git::with_exe("git")
            .env("HOME", &home)
            .env("USERPROFILE", &home);
        let paths = crate::Paths::at(dir.path().join("data"));
        (dir, git, paths)
    }

    #[test]
    fn ensure_known_hosts_creates_the_file() {
        let (_dir, git, paths) = known_hosts_env();
        let path = known_hosts_path(&git).unwrap();
        assert!(!path.exists());
        assert_eq!(
            ensure_github_known_hosts(&git, &paths).unwrap(),
            KnownHostsOutcome::Added { backup: None }
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(known_hosts_trusts_github(&text));
        for line in GITHUB_KNOWN_HOSTS {
            assert!(text.contains(line));
        }
        // Second call: nothing to do, file unchanged.
        assert_eq!(
            ensure_github_known_hosts(&git, &paths).unwrap(),
            KnownHostsOutcome::AlreadyTrusted
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn ensure_known_hosts_keeps_existing_lines_and_backs_up() {
        let (_dir, git, paths) = known_hosts_env();
        let path = known_hosts_path(&git).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let before = "gitlab.com ssh-ed25519 AAAAexample\r\nbitbucket.org ssh-rsa AAAAother";
        std::fs::write(&path, before).unwrap();
        let out = ensure_github_known_hosts(&git, &paths).unwrap();
        let KnownHostsOutcome::Added {
            backup: Some(backup),
        } = out
        else {
            panic!("{out:?}");
        };
        assert_eq!(std::fs::read_to_string(backup).unwrap(), before);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with(&format!("{before}\r\n")), "{after}");
        assert!(after.contains(&format!("{}\r\n", GITHUB_KNOWN_HOSTS[0])));
    }

    #[test]
    fn already_trusted_file_is_not_touched() {
        let (_dir, git, paths) = known_hosts_env();
        let path = known_hosts_path(&git).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{}\n", GITHUB_KNOWN_HOSTS[1])).unwrap();
        assert_eq!(
            ensure_github_known_hosts(&git, &paths).unwrap(),
            KnownHostsOutcome::AlreadyTrusted
        );
        assert!(!paths.backups_dir().exists());
    }

    #[test]
    fn private_key_encryption_detection() {
        use base64::Engine as _;
        let blob = |cipher: &str| {
            let mut raw = b"openssh-key-v1\0".to_vec();
            raw.extend_from_slice(&(cipher.len() as u32).to_be_bytes());
            raw.extend_from_slice(cipher.as_bytes());
            raw.extend_from_slice(&[0, 0, 0, 4, b'n', b'o', b'n', b'e']);
            let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
            format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
                &b64[..20],
                &b64[20..]
            )
        };
        assert_eq!(private_key_is_encrypted(&blob("none")), Some(false));
        assert_eq!(private_key_is_encrypted(&blob("aes256-ctr")), Some(true));
        assert_eq!(
            private_key_is_encrypted(
                "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nxx\n-----END RSA PRIVATE KEY-----"
            ),
            Some(true)
        );
        assert_eq!(
            private_key_is_encrypted(
                "-----BEGIN RSA PRIVATE KEY-----\nMIIx\n-----END RSA PRIVATE KEY-----"
            ),
            Some(false)
        );
        assert_eq!(
            private_key_is_encrypted("-----BEGIN ENCRYPTED PRIVATE KEY-----\nx"),
            Some(true)
        );
        assert_eq!(private_key_is_encrypted("not a key"), None);
        assert_eq!(
            private_key_is_encrypted("-----BEGIN OPENSSH PRIVATE KEY-----\n!!!\n-----END"),
            None
        );
    }

    #[test]
    fn ssh_add_list_parse() {
        let out = "256 SHA256:abc/DEF+1 me@x (ED25519)\r\n3072 SHA256:zzz other (RSA)\n";
        assert_eq!(
            parse_ssh_add_list(out),
            vec!["SHA256:abc/DEF+1".to_string(), "SHA256:zzz".to_string()]
        );
        assert!(parse_ssh_add_list("The agent has no identities.\n").is_empty());
    }

    /// Stub agent: never runs `sc`, `ssh-add` or an elevated process.
    struct FakeAgent {
        running: std::cell::Cell<bool>,
        held: std::cell::RefCell<Vec<String>>,
        add_ok: bool,
        accept_uac: bool,
        to_add: String,
        added_calls: std::cell::Cell<u32>,
        uac_calls: std::cell::Cell<u32>,
    }

    impl FakeAgent {
        fn new(running: bool, to_add: &str) -> Self {
            Self {
                running: std::cell::Cell::new(running),
                held: std::cell::RefCell::new(Vec::new()),
                add_ok: true,
                accept_uac: true,
                to_add: to_add.to_string(),
                added_calls: std::cell::Cell::new(0),
                uac_calls: std::cell::Cell::new(0),
            }
        }
    }

    impl AgentHost for FakeAgent {
        fn service(&self) -> AgentStatus {
            if self.running.get() {
                AgentStatus::Running
            } else {
                AgentStatus::Stopped
            }
        }
        fn fingerprints(&self) -> Option<Vec<String>> {
            self.running.get().then(|| self.held.borrow().clone())
        }
        fn add_key_in_console(&self, _key: &Path) -> Result<bool> {
            self.added_calls.set(self.added_calls.get() + 1);
            if self.add_ok {
                self.held.borrow_mut().push(self.to_add.clone());
            }
            Ok(self.add_ok)
        }
        fn turn_on_service(&self) -> Result<bool> {
            self.uac_calls.set(self.uac_calls.get() + 1);
            if self.accept_uac {
                self.running.set(true);
            }
            Ok(self.accept_uac)
        }
        fn send_agent_request(&self, request: &[u8]) -> Result<bool> {
            assert_eq!(request[4], 17, "SSH2_AGENTC_ADD_IDENTITY");
            self.added_calls.set(self.added_calls.get() + 1);
            if self.add_ok {
                self.held.borrow_mut().push(self.to_add.clone());
            }
            Ok(self.add_ok)
        }
    }

    /// A real encrypted ed25519 key (in-app flow); returns (key, fingerprint).
    fn real_key(dir: &Path, passphrase: &str) -> (PathBuf, String) {
        let key = dir.join("githop_real");
        let public = crate::keyfile::create_ed25519(&key, "t", Some(passphrase)).unwrap();
        (key, public_key_fingerprint(&public).unwrap())
    }

    #[test]
    fn in_app_remember_checks_the_passphrase_before_any_uac_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (key, fp) = real_key(dir.path(), "correct horse");
        // Agent off, wrong passphrase: error, and no UAC prompt was shown.
        let agent = FakeAgent::new(false, &fp);
        assert!(matches!(
            remember_with_passphrase(&agent, &key, Some("nope"), true),
            Err(CoreError::WrongPassphrase)
        ));
        assert_eq!(agent.uac_calls.get(), 0);
        // Right passphrase, agent off, not allowed to turn on: AgentOff.
        assert_eq!(
            remember_with_passphrase(&agent, &key, Some("correct horse"), false).unwrap(),
            RememberOutcome::AgentOff
        );
        // Allowed to turn on: one UAC prompt, then remembered.
        assert_eq!(
            remember_with_passphrase(&agent, &key, Some("correct horse"), true).unwrap(),
            RememberOutcome::Remembered
        );
        assert_eq!(agent.uac_calls.get(), 1);
        assert_eq!(passphrase_state(&agent, &key), PassphraseState::Remembered);
    }

    #[test]
    fn in_app_remember_of_a_key_without_passphrase_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        crate::keyfile::create_ed25519(&key, "t", None).unwrap();
        let agent = FakeAgent::new(true, "x");
        assert_eq!(
            remember_with_passphrase(&agent, &key, None, true).unwrap(),
            RememberOutcome::NoPassphrase
        );
        assert_eq!(agent.added_calls.get(), 0);
    }

    /// An encrypted-looking OpenSSH key plus its `.pub`; returns (key, fingerprint).
    fn encrypted_key(dir: &Path) -> (PathBuf, String) {
        use base64::Engine as _;
        let mut raw = b"openssh-key-v1\0".to_vec();
        raw.extend_from_slice(&10u32.to_be_bytes());
        raw.extend_from_slice(b"aes256-ctr");
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        let key = dir.join("githop_test");
        std::fs::write(
            &key,
            format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n{b64}\n-----END OPENSSH PRIVATE KEY-----\n"
            ),
        )
        .unwrap();
        let pub_line = GITHUB_KNOWN_HOSTS[0]
            .split_once(' ')
            .map(|(_, k)| k)
            .unwrap();
        std::fs::write(pub_path(&key), format!("{pub_line} test\n")).unwrap();
        (key, GITHUB_FINGERPRINTS[0].to_string())
    }

    #[test]
    fn passphrase_flow_with_agent_running() {
        let dir = tempfile::tempdir().unwrap();
        let (key, fp) = encrypted_key(dir.path());
        let agent = FakeAgent::new(true, &fp);
        assert_eq!(
            passphrase_state(&agent, &key),
            PassphraseState::AsksEachTime
        );
        assert_eq!(
            remember_passphrase(&agent, &key).unwrap(),
            RememberOutcome::Remembered
        );
        assert_eq!(passphrase_state(&agent, &key), PassphraseState::Remembered);
        assert_eq!(agent.uac_calls.get(), 0);
        assert_eq!(
            passphrase_state(&agent, &dir.path().join("missing")),
            PassphraseState::NoKey
        );
    }

    #[test]
    fn passphrase_flow_turns_the_agent_on_first() {
        let dir = tempfile::tempdir().unwrap();
        let (key, fp) = encrypted_key(dir.path());
        let agent = FakeAgent::new(false, &fp);
        assert_eq!(passphrase_state(&agent, &key), PassphraseState::AgentOff);
        assert_eq!(
            remember_passphrase(&agent, &key).unwrap(),
            RememberOutcome::AgentOff
        );
        assert_eq!(agent.added_calls.get(), 0);
        assert_eq!(
            turn_on_agent_and_remember(&agent, &key).unwrap(),
            RememberOutcome::Remembered
        );
        assert_eq!(agent.uac_calls.get(), 1);

        let mut declined = FakeAgent::new(false, &fp);
        declined.accept_uac = false;
        assert_eq!(
            turn_on_agent_and_remember(&declined, &key).unwrap(),
            RememberOutcome::Cancelled
        );
        assert_eq!(declined.added_calls.get(), 0);

        let mut wrong = FakeAgent::new(true, &fp);
        wrong.add_ok = false;
        assert_eq!(
            remember_passphrase(&wrong, &key).unwrap(),
            RememberOutcome::NotAdded
        );
    }

    #[test]
    fn key_without_passphrase_needs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        std::fs::write(
            &key,
            "-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        let agent = FakeAgent::new(false, "");
        assert_eq!(
            passphrase_state(&agent, &key),
            PassphraseState::NoPassphrase
        );
        assert_eq!(
            turn_on_agent_and_remember(&agent, &key).unwrap(),
            RememberOutcome::NoPassphrase
        );
        assert_eq!(agent.uac_calls.get(), 0);
    }

    #[test]
    fn elevation_script_is_constant_and_runs_sc_once_elevated() {
        assert!(ELEVATE_AGENT_SCRIPT.contains("-Verb RunAs"));
        assert!(ELEVATE_AGENT_SCRIPT.contains("{0} config ssh-agent start= auto"));
        assert!(ELEVATE_AGENT_SCRIPT.contains("{0} start ssh-agent"));
        assert!(ELEVATE_AGENT_SCRIPT.contains("Join-Path $sys 'sc.exe'"));
        assert!(ELEVATE_AGENT_SCRIPT.contains("/d"));
        assert_eq!(ELEVATE_AGENT_SCRIPT.matches("Start-Process").count(), 1);
        assert!(!ELEVATE_AGENT_SCRIPT.contains('"'));
        // No exe path, elevated or run by the elevated cmd, comes from an
        // environment variable (cmd would expand %...% from HKCU\Environment).
        assert!(ELEVATE_AGENT_SCRIPT.contains("[Environment]::SystemDirectory"));
        assert!(!ELEVATE_AGENT_SCRIPT.contains("$env:"));
        assert!(!ELEVATE_AGENT_SCRIPT.contains('%'));
    }

    #[test]
    fn pub_path_appends() {
        assert_eq!(
            pub_path(Path::new("C:/k/githop_work.v2")),
            PathBuf::from("C:/k/githop_work.v2.pub")
        );
    }

    #[test]
    fn read_public_key_trims_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        assert!(read_public_key(&key).is_err());
        std::fs::write(pub_path(&key), "ssh-ed25519 AAAA test\r\n").unwrap();
        assert_eq!(read_public_key(&key).unwrap(), "ssh-ed25519 AAAA test");
    }

    #[test]
    fn keygen_refuses_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        std::fs::write(&key, "x").unwrap();
        match keygen(&key, "c", true) {
            Err(CoreError::Io { source, .. }) => {
                assert_eq!(source.kind(), io::ErrorKind::AlreadyExists)
            }
            other => panic!("unexpected {other:?}"),
        }
        // Existing .pub alone also refuses.
        let key2 = dir.path().join("k2");
        std::fs::write(pub_path(&key2), "x").unwrap();
        assert!(keygen(&key2, "c", true).is_err());
    }

    #[test]
    fn ssh_config_conflicts_reads_home() {
        let dir = tempfile::tempdir().unwrap();
        let git = Git::with_exe("git")
            .env("HOME", dir.path())
            .env("USERPROFILE", dir.path());
        assert!(ssh_config_conflicts(&git).unwrap().is_empty());
        std::fs::create_dir_all(dir.path().join(".ssh")).unwrap();
        std::fs::write(
            dir.path().join(".ssh").join("config"),
            "Host github.com\n IdentityFile ~/.ssh/id_old\n",
        )
        .unwrap();
        let got = ssh_config_conflicts(&git).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].identity_file, "~/.ssh/id_old");
    }
}
