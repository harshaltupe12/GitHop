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
    let exe = match locate_tool("ssh") {
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
                "Hi harshal-personal! You've successfully authenticated, but GitHub does not provide shell access.\n",
                Some(1),
                ConnectionTest::Connected { user: "harshal-personal".into() },
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
            parse_whoami_sid("\"desktop-x\\harshal\",\"S-1-5-21-390-301-410-1001\"\r\n").as_deref(),
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
