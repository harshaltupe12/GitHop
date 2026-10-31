//! Git remote URL parsing (Plan §7.1).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "resolver" task.
//!
//! Hand-written parser (no regex / url crates): it runs inside every hook, must
//! be allocation-light and must never panic on garbage input.

use serde::{Deserialize, Serialize};

/// Parsed remote URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInfo {
    /// Host as written (lower-cased), e.g. `github.com` or an SSH alias `github-personal`.
    pub host: String,
    /// Owner (user/org), lower-cased.
    pub owner: String,
    /// Repo name without `.git`.
    pub repo: String,
}

/// Hosts that are GitHub without any alias lookup. `ssh.github.com` is
/// GitHub's official SSH-over-443 endpoint.
const GITHUB_HOSTS: [&str; 3] = ["github.com", "www.github.com", "ssh.github.com"];

/// Upper bound on input length; anything longer is not a real remote URL.
const MAX_URL_LEN: usize = 4096;

impl RemoteInfo {
    /// True when the host is `github.com` (or `www.github.com`), or one of the
    /// cached SSH aliases known to resolve to github.com (case-insensitive).
    pub fn is_github(&self, github_aliases: &[String]) -> bool {
        GITHUB_HOSTS
            .iter()
            .any(|h| self.host.eq_ignore_ascii_case(h))
            || github_aliases
                .iter()
                .any(|a| self.host.eq_ignore_ascii_case(a.trim()))
    }
}

/// Parse scp-like (`git@host:owner/repo.git`), `https://[user@]host/owner/repo[.git][/]`,
/// `ssh://[user@]host[:port]/owner/repo[.git]` and `git://` URLs.
/// `file://` and local paths → `None`. Never panics on garbage input.
pub fn parse_remote_url(url: &str) -> Option<RemoteInfo> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return None;
    }
    match url.split_once("://") {
        Some((scheme, rest)) => parse_scheme_url(scheme, rest),
        None => parse_scp_like(url),
    }
}

/// `scheme://[userinfo@]host[:port]/path[?query][#fragment]`.
fn parse_scheme_url(scheme: &str, rest: &str) -> Option<RemoteInfo> {
    let known = ["https", "http", "ssh", "git", "git+ssh", "ssh+git"];
    if !known.iter().any(|k| scheme.eq_ignore_ascii_case(k)) {
        // Includes `file://` and anything unknown.
        return None;
    }
    let (authority, path) = rest.split_once('/')?;
    let host_port = strip_userinfo(authority);
    let host = match host_port.split_once(':') {
        Some((host, port)) => {
            if !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            host
        }
        None => host_port,
    };
    let path = path.split(['?', '#']).next().unwrap_or_default();
    build(host, path)
}

/// `[user@]host:path` — git's rule: a `:` before any `/` means scp-like,
/// except a single-letter "host" which is a Windows drive (`C:\x`, `C:/x`).
fn parse_scp_like(url: &str) -> Option<RemoteInfo> {
    let (left, path) = url.split_once(':')?;
    if left.contains(['/', '\\']) {
        return None; // local path such as `./a:b` or `C\x:y`
    }
    if left.len() == 1 && left.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None; // drive letter
    }
    build(strip_userinfo(left), path)
}

/// Drop `user@` / `user:password@` (credentials are never kept).
fn strip_userinfo(authority: &str) -> &str {
    match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    }
}

fn build(host: &str, path: &str) -> Option<RemoteInfo> {
    if !valid_host(host) {
        return None;
    }
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let owner = segments.next()?;
    let mut last = None;
    for seg in segments {
        if !valid_segment(seg) {
            return None;
        }
        last = Some(seg);
    }
    let last = last?;
    if !valid_segment(owner) {
        return None;
    }
    let repo = strip_dot_git(last);
    if repo.is_empty() || repo == "." || repo == ".." || repo.eq_ignore_ascii_case(".git") {
        return None;
    }
    Some(RemoteInfo {
        host: host.to_ascii_lowercase(),
        owner: owner.to_ascii_lowercase(),
        repo: repo.to_string(),
    })
}

/// Hostname or SSH alias: ASCII letters, digits, `-`, `.`, `_`. A leading `-`
/// is rejected so a host can never be read as an ssh option.
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
}

/// One path segment: ASCII letters, digits, `-`, `.`, `_`, `~`; not `.`/`..`.
fn valid_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

fn strip_dot_git(name: &str) -> &str {
    let n = name.len();
    if n > 4 && name.is_char_boundary(n - 4) && name[n - 4..].eq_ignore_ascii_case(".git") {
        &name[..n - 4]
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ri(host: &str, owner: &str, repo: &str) -> Option<RemoteInfo> {
        Some(RemoteInfo {
            host: host.into(),
            owner: owner.into(),
            repo: repo.into(),
        })
    }

    #[test]
    fn url_table() {
        let gh = |o: &str, r: &str| ri("github.com", o, r);
        let cases: Vec<(&str, Option<RemoteInfo>)> = vec![
            // scp-like
            ("git@github.com:Owner/repo.git", gh("owner", "repo")),
            ("git@github.com:owner/repo", gh("owner", "repo")),
            ("git@github.com:owner/repo.git/", gh("owner", "repo")),
            ("git@github.com:/owner/repo.git", gh("owner", "repo")),
            ("github.com:owner/repo.git", gh("owner", "repo")),
            ("git@GitHub.COM:ACME/Repo.git", gh("acme", "Repo")),
            ("  git@github.com:owner/repo.git\n", gh("owner", "repo")),
            (
                "git@github-personal:owner/repo.git",
                ri("github-personal", "owner", "repo"),
            ),
            ("git@gh_work:acme/api.git", ri("gh_work", "acme", "api")),
            // https
            ("https://github.com/owner/repo", gh("owner", "repo")),
            ("https://github.com/owner/repo.git", gh("owner", "repo")),
            ("https://github.com/owner/repo/", gh("owner", "repo")),
            ("https://github.com/owner/repo.git/", gh("owner", "repo")),
            ("https://github.com//owner//repo", gh("owner", "repo")),
            ("HTTPS://GITHUB.COM/Owner/Repo.GIT", gh("owner", "Repo")),
            ("http://github.com/owner/repo", gh("owner", "repo")),
            (
                "https://user@github.com/owner/repo.git",
                gh("owner", "repo"),
            ),
            (
                "https://user:pw@github.com/owner/repo.git",
                gh("owner", "repo"),
            ),
            (
                "https://user:p@ss@github.com/owner/repo.git",
                gh("owner", "repo"),
            ),
            ("https://github.com:443/owner/repo.git", gh("owner", "repo")),
            (
                "https://www.github.com/owner/repo",
                ri("www.github.com", "owner", "repo"),
            ),
            (
                "https://github.com/owner/repo?x=1#frag",
                gh("owner", "repo"),
            ),
            (
                "https://github.com/owner/my.repo.js.git",
                gh("owner", "my.repo.js"),
            ),
            // ssh:// and git://
            ("ssh://git@github.com/owner/repo.git", gh("owner", "repo")),
            (
                "ssh://git@github.com:22/owner/repo.git",
                gh("owner", "repo"),
            ),
            ("ssh://github.com/owner/repo", gh("owner", "repo")),
            (
                "ssh://git@ssh.github.com:443/owner/repo.git",
                ri("ssh.github.com", "owner", "repo"),
            ),
            (
                "git+ssh://git@github.com/owner/repo.git",
                gh("owner", "repo"),
            ),
            ("git://github.com/owner/repo.git", gh("owner", "repo")),
            // other hosts / subgroups
            (
                "git@gitlab.com:group/sub/repo.git",
                ri("gitlab.com", "group", "repo"),
            ),
            (
                "https://gitlab.example.com/a/b/c/d.git",
                ri("gitlab.example.com", "a", "d"),
            ),
            (
                "https://bitbucket.org/team/repo",
                ri("bitbucket.org", "team", "repo"),
            ),
            // local paths / file
            ("file:///C:/repos/x.git", None),
            ("file://server/share/x", None),
            (r"C:\repos\x", None),
            ("C:/repos/x", None),
            ("c:repos/x", None),
            ("/srv/git/x.git", None),
            ("../x", None),
            ("./a:b/c", None),
            (r"\\server\share\x", None),
            // garbage
            ("", None),
            ("   ", None),
            ("github.com", None),
            ("git@github.com:", None),
            ("git@github.com:owner", None),
            ("git@github.com:owner/", None),
            ("https://github.com/owner", None),
            ("https://github.com", None),
            ("https:///owner/repo", None),
            ("https://github.com:abc/owner/repo", None),
            ("ftp://github.com/owner/repo", None),
            ("://github.com/owner/repo", None),
            ("git@:owner/repo", None),
            ("git@-oProxyCommand=x:owner/repo", None),
            ("git@github.com:owner/.git", None),
            ("git@github.com:owner/..", None),
            ("git@github.com:owner/x.git.git", gh("owner", "x.git")),
            ("git@github.com:own er/repo", None),
            ("https://gïthub.com/owner/repo", None),
            ("https://github.com/ownér/repo", None),
            ("git@github.com:owner/repo name.git", None),
            ("日本語", None),
        ];
        assert!(cases.len() >= 30);
        for (url, want) in cases {
            assert_eq!(parse_remote_url(url), want, "url: {url:?}");
        }
    }

    #[test]
    fn overlong_input_is_rejected() {
        let long = format!("git@github.com:owner/{}.git", "r".repeat(MAX_URL_LEN));
        assert_eq!(parse_remote_url(&long), None);
    }

    #[test]
    fn is_github_hosts_and_aliases() {
        let mk = |h: &str| RemoteInfo {
            host: h.into(),
            owner: "o".into(),
            repo: "r".into(),
        };
        let aliases = vec!["GitHub-Personal".to_string(), " gh-work ".to_string()];
        assert!(mk("github.com").is_github(&[]));
        assert!(mk("www.github.com").is_github(&[]));
        assert!(mk("ssh.github.com").is_github(&[]));
        assert!(!mk("gitlab.com").is_github(&aliases));
        assert!(!mk("github-personal").is_github(&[]));
        assert!(mk("github-personal").is_github(&aliases));
        assert!(mk("gh-work").is_github(&aliases));
        assert!(!mk("notgithub.com").is_github(&aliases));
    }

    /// Deterministic pseudo-fuzz: random strings from a URL-ish alphabet plus
    /// every prefix/suffix of valid URLs. Must never panic; any `Some` result
    /// must satisfy the output invariants.
    #[test]
    fn pseudo_fuzz_never_panics() {
        const ALPHABET: &[&str] = &[
            "git",
            "@",
            ":",
            "/",
            "//",
            "://",
            ".",
            ".git",
            "github.com",
            "ssh",
            "https",
            "file",
            "\\",
            "C",
            "~",
            "-",
            "_",
            "?",
            "#",
            " ",
            "\t",
            "\0",
            "é",
            "日",
            "🦀",
            "%2F",
            "[",
            "]",
            "a",
            "Z",
            "9",
            "22",
            "owner",
            "repo",
        ];
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            // xorshift64*
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let check = |s: &str| {
            if let Some(r) = parse_remote_url(s) {
                assert!(
                    !r.host.is_empty() && !r.owner.is_empty() && !r.repo.is_empty(),
                    "{s:?}"
                );
                assert_eq!(r.owner, r.owner.to_ascii_lowercase(), "{s:?}");
                assert_eq!(r.host, r.host.to_ascii_lowercase(), "{s:?}");
            }
        };
        for _ in 0..5000 {
            let len = (next() % 12) as usize;
            let mut s = String::new();
            for _ in 0..len {
                s.push_str(ALPHABET[(next() % ALPHABET.len() as u64) as usize]);
            }
            check(&s);
        }
        let seeds = [
            "ssh://git@github.com:22/Owner/repo.git",
            "https://user:pw@github.com/owner/repo.git/",
            "git@github-personal:owner/repo.git",
            "file:///C:/x/y.git",
            "ssh://日本@ホスト/🦀/r",
        ];
        for seed in seeds {
            for (i, _) in seed.char_indices() {
                check(&seed[..i]);
                check(&seed[i..]);
            }
        }
    }
}
