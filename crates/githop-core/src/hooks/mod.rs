//! Global git hooks: render, install, uninstall, verify (Phase 3 §3.1–3.2, §4.1;
//! Plan §7.7, §8 C2/C4).
//!
//! GitHop owns `<data dir>/hooks/` and points global `core.hooksPath` at it.
//! Three hooks run a check (`pre-commit`, `pre-push`, `post-checkout`); every
//! other hook git knows is a pass-through stub, because a global
//! `core.hooksPath` would otherwise hide each repo's own `.git/hooks/` (C2).
//! Every script chains to `<git common dir>/hooks/<name>` and then to the
//! user's previous global hooks path (C4).
//!
//! Scripts are POSIX `sh` (run by Git for Windows' bundled `sh`), LF only, no
//! BOM. `githop.exe` is embedded as an absolute, quoted path, so hooks never
//! depend on `PATH`. Exit semantics inside the scripts: githop exit 1 → block;
//! 3 → continue (githop already printed the warning); anything else → print the
//! fail-open warning and continue. `post-checkout` never blocks.
//!
//! Callers ([`install_hooks`] / [`uninstall_hooks`]) must back up the global git
//! config first (`backup::backup_global_gitconfig`) and save `settings`
//! afterwards (`Store::save_settings`); this module does neither.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::gitconfig;
use crate::paths::{self, Paths};
use crate::settings::Settings;

/// Global git config key GitHop sets.
pub const HOOKS_PATH_KEY: &str = "core.hooksPath";

/// Hooks that run a GitHop check.
pub const CHECKED_HOOKS: [&str; 3] = ["pre-commit", "pre-push", "post-checkout"];

/// Hooks that only chain. Spec §3.1 list, plus the server-side and git-p4 hooks
/// (a global `core.hooksPath` also applies to local bare repos receiving a push,
/// e.g. `file://` remotes, and to git-p4).
pub const PASSTHROUGH_HOOKS: [&str; 25] = [
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-merge",
    "post-rewrite",
    "reference-transaction",
    "post-index-change",
    "fsmonitor-watchman",
    "sendemail-validate",
    "pre-auto-gc",
    "push-to-checkout",
    "pre-receive",
    "update",
    "proc-receive",
    "post-receive",
    "post-update",
    "p4-changelist",
    "p4-prepare-changelist",
    "p4-post-changelist",
    "p4-pre-submit",
];

/// Pass-through hooks that receive data on stdin (githooks(5)). When both a repo
/// hook and a previous global hook exist, the stub buffers stdin once and feeds
/// it to both; otherwise stdin is inherited directly. `proc-receive` is not
/// here on purpose: it speaks an interactive protocol and must never be buffered.
pub const STDIN_HOOKS: [&str; 4] = [
    "post-rewrite",
    "reference-transaction",
    "pre-receive",
    "post-receive",
];

const TPL_PRE_COMMIT: &str = include_str!("templates/pre-commit.sh");
const TPL_PRE_PUSH: &str = include_str!("templates/pre-push.sh");
const TPL_POST_CHECKOUT: &str = include_str!("templates/post-checkout.sh");
const TPL_PASSTHROUGH: &str = include_str!("templates/passthrough.sh");
const TPL_LOCATE: &str = include_str!("templates/locate.sh");
const TPL_CHAIN: &str = include_str!("templates/chain.sh");

const BUFFER_SNIPPET: &str = "# Both hooks need stdin: buffer it once.\n\
if [ -n \"$REPO_HOOK\" ] && [ -n \"$PREV_HOOK\" ]; then\n  INPUT=\"$(cat)\"\n  BUFFERED=1\nfi\n";

/// Pass-through hooks git runs very often (`reference-transaction` ~5× per
/// commit, `post-index-change` on every index write) but that almost no repo
/// uses. Each stub costs an `sh` start (~30–50 ms on Windows), so they are only
/// installed when `settings.all_hook_stubs` is set or the previous global hooks
/// folder has them. `doctor` warns when a pinned repo has its own copy.
pub const OPTIONAL_HOOKS: [&str; 2] = ["reference-transaction", "post-index-change"];

/// Every hook file name GitHop knows, checked hooks first.
pub fn all_hook_names() -> Vec<&'static str> {
    CHECKED_HOOKS
        .iter()
        .chain(PASSTHROUGH_HOOKS.iter())
        .copied()
        .collect()
}

/// Hook file names to install: all of them with `all_stubs`, else all minus
/// [`OPTIONAL_HOOKS`].
pub fn installed_hook_names(all_stubs: bool) -> Vec<&'static str> {
    all_hook_names()
        .into_iter()
        .filter(|n| all_stubs || !OPTIONAL_HOOKS.contains(n))
        .collect()
}

/// Whether the optional stubs are wanted: forced by settings, or needed to keep
/// chaining a previous global hooks folder that has one of them.
fn wants_all_stubs(settings: &Settings, previous_expanded: &str) -> bool {
    settings.all_hook_stubs
        || (!previous_expanded.is_empty()
            && OPTIONAL_HOOKS
                .iter()
                .any(|h| Path::new(previous_expanded).join(h).is_file()))
}

/// Escape a value for use inside an sh double-quoted string.
///
/// Backslashes become `/` first (Windows paths), then `"`, `$` and backtick are
/// backslash-escaped. Control characters (newline, CR, NUL, …) are rejected:
/// they can't appear in a real Windows path and would break the script.
pub fn sh_quote_value(value: &str) -> Result<String> {
    if let Some(c) = value.chars().find(|c| c.is_control()) {
        return Err(invalid_input(format!(
            "this path contains a control character ({:?}), so GitHop can't put it in a hook",
            c
        )));
    }
    let slashed = value.replace('\\', "/");
    let mut out = String::with_capacity(slashed.len() + 4);
    for c in slashed.chars() {
        if matches!(c, '"' | '$' | '`' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    Ok(out)
}

fn invalid_input(msg: String) -> CoreError {
    CoreError::io(
        PathBuf::new(),
        std::io::Error::new(std::io::ErrorKind::InvalidInput, msg),
    )
}

/// Replace `@@NAME@@` tokens in one pass (substituted values are never re-scanned).
fn substitute(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let replaced = after.find("@@").and_then(|end| {
            let name = &after[..end];
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (end, *v))
        });
        match replaced {
            Some((end, value)) => {
                out.push_str(value);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("@@");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn lf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Expand a leading `~/` in a hooks path like git does (using `home`).
pub fn expand_home(path: &str, home: Option<&Path>) -> String {
    match (
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")),
        home,
    ) {
        (Some(rest), Some(home)) => {
            format!("{}/{}", paths::to_slash(home).trim_end_matches('/'), rest)
        }
        _ => path.to_string(),
    }
}

/// Render one hook script. Pure.
///
/// * `githop_exe` — absolute path of `githop.exe` (any slash style).
/// * `previous` — previous global hooks path to chain to, already `~`-expanded;
///   empty when none.
///
/// Output is LF-only, starts with `#!/bin/sh`, and has no BOM.
pub fn render_hook(name: &str, githop_exe: &str, previous: &str) -> Result<String> {
    let template = match name {
        "pre-commit" => TPL_PRE_COMMIT,
        "pre-push" => TPL_PRE_PUSH,
        "post-checkout" => TPL_POST_CHECKOUT,
        n if all_hook_names().contains(&n) => TPL_PASSTHROUGH,
        other => return Err(invalid_input(format!("{other} is not a hook GitHop knows"))),
    };
    if CHECKED_HOOKS.contains(&name) && !paths::is_absolute_str(githop_exe) {
        return Err(invalid_input(format!(
            "githop path must be absolute, got {githop_exe}"
        )));
    }
    let exe = sh_quote_value(githop_exe)?;
    let prev = sh_quote_value(previous.trim_end_matches(['/', '\\']))?;
    let buffer = if STDIN_HOOKS.contains(&name) {
        BUFFER_SNIPPET
    } else {
        ""
    };
    let locate = lf(TPL_LOCATE);
    let chain = lf(TPL_CHAIN);
    let rendered = substitute(
        &lf(template),
        &[
            ("GITHOP", &exe),
            ("PREVIOUS", &prev),
            ("HOOK", name),
            ("LOCATE", locate.trim_end()),
            ("CHAIN", chain.trim_end()),
            ("BUFFER", buffer),
        ],
    );
    Ok(rendered.trim_start_matches('\u{feff}').to_string())
}

/// Render every hook: `(file name, contents)`, in [`all_hook_names`] order.
pub fn render_all(githop_exe: &str, previous: &str) -> Result<Vec<(String, String)>> {
    render_names(&all_hook_names(), githop_exe, previous)
}

/// Render the given hooks: `(file name, contents)`, in the given order.
pub fn render_names(
    names: &[&str],
    githop_exe: &str,
    previous: &str,
) -> Result<Vec<(String, String)>> {
    names
        .iter()
        .map(|n| render_hook(n, githop_exe, previous).map(|c| (n.to_string(), c)))
        .collect()
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// One line to paste at the top of a repo's own hook (husky, lefthook…) when the
/// repo sets a local `core.hooksPath` (C3). Fails open like the GitHop hooks:
/// only exit 1 (wrong identity) blocks.
pub fn snippet(stage: &str, githop_exe: &str) -> Result<String> {
    let stage = match stage {
        "commit" | "pre-commit" => "commit",
        "push" | "pre-push" => "push",
        "clone" | "post-checkout" => "clone",
        other => return Err(invalid_input(format!("unknown stage {other}"))),
    };
    let exe = sh_quote_value(githop_exe)?;
    Ok(format!(
        "\"{exe}\" check --stage {stage} -- \"$@\"; [ $? -ne 1 ] || exit 1"
    ))
}

/// GitHop's hooks dir as git should see it (forward slashes, no trailing `/`).
pub fn hooks_dir_value(paths: &Paths) -> String {
    paths::to_slash(&paths.hooks_dir())
        .trim_end_matches('/')
        .to_string()
}

/// True when two hooks paths name the same folder (slashes, case, trailing `/`).
pub fn same_dir(a: &str, b: &str) -> bool {
    let key = |s: &str| {
        let canon = dunce::canonicalize(s)
            .map(|p| paths::to_slash(&p))
            .unwrap_or_else(|_| s.to_string());
        paths::normalize_folder(&canon)
            .map(|n| paths::folder_key(&n))
            .unwrap_or_else(|| {
                canon
                    .replace('\\', "/")
                    .trim_end_matches('/')
                    .to_lowercase()
            })
    };
    key(a) == key(b)
}

/// What [`install_hooks`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReport {
    /// Value written to global `core.hooksPath`.
    pub hooks_dir: String,
    /// Previous global hooks path GitHop chains to (and restores on uninstall).
    pub previous_hooks_path: Option<String>,
    /// File names written.
    pub written: Vec<String>,
    /// Old GitHop hook files removed (no longer generated).
    pub removed: Vec<String>,
    /// True when hooks were already installed (re-install / upgrade).
    pub reinstall: bool,
}

/// Write `contents` to `path` atomically (temp file in the same dir + rename).
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut last_err = None;
    for _ in 0..3 {
        let mut tmp = tempfile::Builder::new()
            .prefix(".githop-hook-")
            .tempfile_in(dir)
            .map_err(|e| CoreError::io(dir, e))?;
        tmp.write_all(contents)
            .map_err(|e| CoreError::io(tmp.path(), e))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| CoreError::io(tmp.path(), e))?;
        // tempfile creates 0600; git skips hooks without the exec bit on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
                .map_err(|e| CoreError::io(tmp.path(), e))?;
        }
        match tmp.persist(path) {
            Ok(_) => return Ok(()),
            Err(e) => last_err = Some(e.error),
        }
    }
    Err(CoreError::io(
        path,
        last_err.unwrap_or_else(|| std::io::Error::other("rename failed")),
    ))
}

/// Install (or re-install) the hooks. See the module docs for what the caller
/// must do before (back up global git config) and after (save `settings`).
///
/// 1. Read global `core.hooksPath`. If set and not GitHop's dir → record it in
///    `settings.previous_hooks_path`. If it *is* GitHop's dir (re-install), keep
///    the recorded previous path, so hooks never chain to themselves.
/// 2. Render every script (fails before touching anything if the exe path is unusable).
/// 3. Write each file atomically; remove old GitHop hook files no longer generated.
/// 4. Set global `core.hooksPath` to the hooks dir (forward slashes).
/// 5. Record SHA-256 per file in `settings.hook_hashes`, set `hooks_installed`
///    and `githop_exe`.
pub fn install_hooks(
    paths: &Paths,
    git: &Git,
    settings: &mut Settings,
    githop_exe: &Path,
) -> Result<InstallReport> {
    let ours = hooks_dir_value(paths);
    let exe = paths::to_slash(githop_exe);
    let current = gitconfig::get_global(git, HOOKS_PATH_KEY)?.filter(|v| !v.trim().is_empty());
    let reinstall =
        settings.hooks_installed || current.as_deref().is_some_and(|c| same_dir(c, &ours));
    let previous = match current.as_deref() {
        Some(c) if same_dir(c, &ours) => settings.previous_hooks_path.clone(),
        Some(c) => c.to_string(),
        None => String::new(),
    };
    // Never chain to ourselves, whatever settings say.
    let previous = if !previous.is_empty() && same_dir(&previous, &ours) {
        String::new()
    } else {
        previous
    };
    let home = git.home_dir();
    let prev_expanded = expand_home(&previous, home.as_deref());
    let names = installed_hook_names(wants_all_stubs(settings, &prev_expanded));
    let rendered = render_names(&names, &exe, &prev_expanded)?;

    let dir = paths.hooks_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let mut hashes = BTreeMap::new();
    let mut written = Vec::new();
    for (name, contents) in &rendered {
        write_atomic(&dir.join(name), contents.as_bytes())?;
        hashes.insert(name.clone(), sha256_hex(contents.as_bytes()));
        written.push(name.clone());
    }
    let mut removed = Vec::new();
    for old in settings.hook_hashes.keys() {
        if !hashes.contains_key(old) && is_safe_file_name(old) {
            let p = dir.join(old);
            if p.is_file() && std::fs::remove_file(&p).is_ok() {
                removed.push(old.clone());
            }
        }
    }

    gitconfig::set_global(git, HOOKS_PATH_KEY, &ours)?;

    settings.previous_hooks_path = previous.clone();
    settings.hook_hashes = hashes;
    settings.hooks_installed = true;
    settings.githop_exe = exe;
    Ok(InstallReport {
        hooks_dir: ours,
        previous_hooks_path: (!previous.is_empty()).then_some(previous),
        written,
        removed,
        reinstall,
    })
}

fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What [`uninstall_hooks`] did to global `core.hooksPath`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HooksPathChange {
    /// Restored the user's previous value.
    Restored { value: String },
    /// Unset (there was no previous value).
    Unset,
    /// Someone else changed it after install; left as is.
    LeftAlone { value: String },
}

/// What [`uninstall_hooks`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallReport {
    pub hooks_path: HooksPathChange,
    /// Hook files removed.
    pub removed: Vec<String>,
}

/// Remove the hooks. Global `core.hooksPath`: if it still points at GitHop (or is
/// unset) → restore `settings.previous_hooks_path` exactly, or unset it when
/// there was none; if the user changed it to something else → leave it alone.
/// Deletes GitHop's hook files (and the dir when empty), then clears
/// `hooks_installed`, `hook_hashes` and `previous_hooks_path`.
/// Caller backs up global git config first and saves `settings` afterwards.
pub fn uninstall_hooks(
    paths: &Paths,
    git: &Git,
    settings: &mut Settings,
) -> Result<UninstallReport> {
    let ours = hooks_dir_value(paths);
    let current = gitconfig::get_global(git, HOOKS_PATH_KEY)?.filter(|v| !v.trim().is_empty());
    let change = match current {
        Some(c) if !same_dir(&c, &ours) => HooksPathChange::LeftAlone { value: c },
        _ => {
            if settings.previous_hooks_path.is_empty() {
                gitconfig::unset_global(git, HOOKS_PATH_KEY)?;
                HooksPathChange::Unset
            } else {
                gitconfig::set_global(git, HOOKS_PATH_KEY, &settings.previous_hooks_path)?;
                HooksPathChange::Restored {
                    value: settings.previous_hooks_path.clone(),
                }
            }
        }
    };

    let dir = paths.hooks_dir();
    let mut names: Vec<String> = all_hook_names().iter().map(|s| s.to_string()).collect();
    for k in settings.hook_hashes.keys() {
        if is_safe_file_name(k) && !names.contains(k) {
            names.push(k.clone());
        }
    }
    let mut removed = Vec::new();
    for name in names {
        let p = dir.join(&name);
        if p.is_file() {
            std::fs::remove_file(&p).map_err(|e| CoreError::io(&p, e))?;
            removed.push(name);
        }
    }
    // Only removes the dir when empty; ignore "not empty" / "missing".
    let _ = std::fs::remove_dir(&dir);

    settings.hooks_installed = false;
    settings.hook_hashes.clear();
    settings.previous_hooks_path.clear();
    Ok(UninstallReport {
        hooks_path: change,
        removed,
    })
}

/// Hook health for `doctor` (Phase 3 §4.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookHealth {
    /// `settings.hooks_installed`.
    pub installed: bool,
    /// GitHop's hooks dir (forward slashes).
    pub hooks_dir: String,
    /// Current global `core.hooksPath`, if set.
    pub global_hooks_path: Option<String>,
    /// Global `core.hooksPath` points at GitHop's dir.
    pub points_to_us: bool,
    /// Hook files that should exist but don't.
    pub missing: Vec<String>,
    /// Files whose SHA-256 differs from the one recorded at install (edited by hand).
    pub tampered: Vec<String>,
    /// Files unchanged since install but older than this GitHop's templates.
    pub stale: Vec<String>,
    /// `githop.exe` path embedded in the hooks.
    pub githop_exe: String,
    /// That exe no longer exists.
    pub exe_missing: bool,
    /// Previous global hooks path being chained to (empty = none).
    pub previous_hooks_path: String,
    /// It no longer exists (chaining silently does nothing).
    pub previous_missing: bool,
    /// Git for Windows `sh` found next to git (None = not found).
    pub sh_path: Option<PathBuf>,
    /// Reading the global config failed.
    pub config_error: Option<String>,
}

impl HookHealth {
    /// No problems at all.
    pub fn is_ok(&self) -> bool {
        self.installed
            && self.points_to_us
            && self.missing.is_empty()
            && self.tampered.is_empty()
            && self.stale.is_empty()
            && !self.exe_missing
            && !self.previous_missing
            && self.sh_path.is_some()
            && self.config_error.is_none()
    }

    /// A re-install (`githop init` / "Repair") would fix what's wrong.
    pub fn needs_reinstall(&self) -> bool {
        self.installed
            && (!self.points_to_us
                || !self.missing.is_empty()
                || !self.tampered.is_empty()
                || !self.stale.is_empty())
    }
}

/// Check the installed hooks. Never fails; problems are reported in the struct.
/// One git spawn (`git config --global --get core.hooksPath`).
pub fn verify_hooks(paths: &Paths, git: &Git, settings: &Settings) -> HookHealth {
    let ours = hooks_dir_value(paths);
    let mut h = HookHealth {
        installed: settings.hooks_installed,
        hooks_dir: ours.clone(),
        githop_exe: settings.githop_exe.clone(),
        previous_hooks_path: settings.previous_hooks_path.clone(),
        sh_path: find_sh(git),
        ..HookHealth::default()
    };
    match gitconfig::get_global(git, HOOKS_PATH_KEY) {
        Ok(v) => {
            let v = v.filter(|s| !s.trim().is_empty());
            h.points_to_us = v.as_deref().is_some_and(|c| same_dir(c, &ours));
            h.global_hooks_path = v;
        }
        Err(e) => h.config_error = Some(e.to_string()),
    }
    h.exe_missing = settings.githop_exe.is_empty() || !Path::new(&settings.githop_exe).is_file();
    let home = git.home_dir();
    let prev_expanded = expand_home(&settings.previous_hooks_path, home.as_deref());
    h.previous_missing = !settings.previous_hooks_path.is_empty()
        && paths::is_absolute_str(&prev_expanded)
        && !Path::new(&prev_expanded).is_dir();

    if !settings.hooks_installed {
        return h;
    }
    let names = installed_hook_names(wants_all_stubs(settings, &prev_expanded));
    let expected: BTreeMap<String, String> =
        render_names(&names, &settings.githop_exe, &prev_expanded)
            .map(|v| {
                v.into_iter()
                    .map(|(n, c)| (n, sha256_hex(c.as_bytes())))
                    .collect()
            })
            .unwrap_or_default();
    let dir = paths.hooks_dir();
    for name in names {
        let p = dir.join(name);
        let Ok(bytes) = std::fs::read(&p) else {
            h.missing.push(name.to_string());
            continue;
        };
        let actual = sha256_hex(&bytes);
        match settings.hook_hashes.get(name) {
            Some(recorded) if *recorded == actual => {
                if expected.get(name).is_some_and(|e| *e != actual) {
                    h.stale.push(name.to_string());
                }
            }
            _ => {
                // No recorded hash, or it differs. If it matches what this
                // GitHop would write, the settings are just out of date.
                if expected.get(name) != Some(&actual) {
                    h.tampered.push(name.to_string());
                }
            }
        }
    }
    h
}

/// Find Git for Windows' `sh.exe` from the git executable's location
/// (`<Git>/cmd/git.exe` or `<Git>/mingw64/bin/git.exe` → `<Git>/bin/sh.exe` or
/// `<Git>/usr/bin/sh.exe`). Elsewhere: `/bin/sh`.
pub fn find_sh(git: &Git) -> Option<PathBuf> {
    if !cfg!(windows) {
        let p = PathBuf::from("/bin/sh");
        return p.exists().then_some(p);
    }
    let exe = if git.exe().is_absolute() {
        git.exe().to_path_buf()
    } else {
        crate::git::find_on_path("git")?
    };
    exe.ancestors().skip(1).take(4).find_map(|dir| {
        ["bin/sh.exe", "usr/bin/sh.exe"]
            .iter()
            .map(|rel| dir.join(rel))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\Harshal\AppData\Local\Programs\GitHop\githop.exe";

    #[test]
    fn names_are_unique_and_complete() {
        let names = all_hook_names();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert_eq!(names.len(), 3 + 25);
        for spec in [
            "commit-msg",
            "prepare-commit-msg",
            "post-commit",
            "pre-rebase",
            "post-merge",
            "post-rewrite",
            "pre-merge-commit",
            "pre-applypatch",
            "applypatch-msg",
            "post-applypatch",
            "reference-transaction",
            "push-to-checkout",
            "post-index-change",
            "fsmonitor-watchman",
            "sendemail-validate",
            "pre-auto-gc",
        ] {
            assert!(names.contains(&spec), "{spec} missing");
        }
        for s in STDIN_HOOKS {
            assert!(names.contains(&s));
        }
    }

    #[test]
    fn rendered_scripts_are_lf_no_bom_with_shebang() {
        for (name, body) in render_all(EXE, "C:/Users/Harshal/.git-hooks").unwrap() {
            assert!(body.starts_with("#!/bin/sh\n"), "{name}");
            assert!(!body.contains('\r'), "{name} has CR");
            assert!(!body.contains('\u{feff}'), "{name} has BOM");
            assert!(!body.contains("@@"), "{name} has unrendered token");
            assert!(
                body.ends_with("exit 0\n") || body.ends_with("exit 0"),
                "{name}"
            );
            assert!(body.contains(&format!("HOOK={name}\n")), "{name}");
            assert!(
                body.contains("PREV=\"C:/Users/Harshal/.git-hooks\"\n"),
                "{name}"
            );
            assert!(body.contains("git rev-parse --git-common-dir"), "{name}");
        }
    }

    #[test]
    fn pre_commit_golden_lines() {
        let s = render_hook("pre-commit", EXE, "").unwrap();
        assert!(
            s.contains("GITHOP=\"C:/Users/Harshal/AppData/Local/Programs/GitHop/githop.exe\"\n")
        );
        assert!(s.contains("PREV=\"\"\n"));
        assert!(s.contains("\"$GITHOP\" check --stage commit -- \"$@\"\n"));
        assert!(s.contains("if [ $rc -eq 1 ]; then exit 1; fi"));
        assert!(s.contains(
            "echo \"! GitHop couldn't check this commit (it went through). Run: githop doctor\" >&2"
        ));
        assert!(s.contains("if [ \"$GITHOP_DISABLE\" != \"1\" ]; then"));
        assert!(!s.contains("$(cat)"), "pre-commit must not read stdin");
    }

    #[test]
    fn pre_push_buffers_stdin_only_when_chaining() {
        let s = render_hook("pre-push", EXE, "").unwrap();
        assert!(s.contains("INPUT=\"$(cat)\""));
        assert!(s.contains("if [ -n \"$REPO_HOOK\" ] || [ -n \"$PREV_HOOK\" ]; then"));
        assert!(s.contains("| \"$GITHOP\" check --stage push -- \"$@\""));
        assert!(s.contains("couldn't check this push"));
    }

    #[test]
    fn post_checkout_only_on_null_sha_and_never_blocks() {
        let s = render_hook("post-checkout", EXE, "").unwrap();
        assert!(s.contains(&format!("{}|{})", "0".repeat(40), "0".repeat(64))));
        assert!(s.contains("check --stage clone"));
        assert!(!s.contains("then exit 1"));
    }

    #[test]
    fn passthrough_stdin_variants() {
        let rt = render_hook("reference-transaction", EXE, "").unwrap();
        assert!(rt.contains("INPUT=\"$(cat)\""));
        let cm = render_hook("commit-msg", EXE, "").unwrap();
        assert!(!cm.contains("$(cat)"));
        assert!(!cm.contains("GITHOP="), "stubs don't need githop");
        let pr = render_hook("proc-receive", EXE, "").unwrap();
        assert!(!pr.contains("$(cat)"), "proc-receive must stream stdin");
    }

    #[test]
    fn escaping_and_rejection() {
        assert_eq!(sh_quote_value(r"C:\a b\c").unwrap(), "C:/a b/c");
        assert_eq!(sh_quote_value("C:/x$y").unwrap(), "C:/x\\$y");
        assert_eq!(sh_quote_value("C:/x`id`").unwrap(), "C:/x\\`id\\`");
        assert_eq!(sh_quote_value("C:/x\"y").unwrap(), "C:/x\\\"y");
        assert!(sh_quote_value("C:/x\ny").is_err());
        assert!(sh_quote_value("C:/x\ry").is_err());
        assert!(sh_quote_value("C:/x\0y").is_err());
        let s = render_hook("pre-commit", "C:/Users/a$b/githop.exe", "").unwrap();
        assert!(s.contains("GITHOP=\"C:/Users/a\\$b/githop.exe\""));
        assert!(render_hook("pre-commit", "C:/a\nrm -rf /", "").is_err());
        assert!(render_hook("pre-commit", "relative/githop.exe", "").is_err());
        assert!(render_hook("not-a-hook", EXE, "").is_err());
    }

    #[test]
    fn substitute_does_not_rescan_values() {
        let out = substitute("a=@@A@@ b=@@B@@ @@X@@", &[("A", "@@B@@"), ("B", "2")]);
        assert_eq!(out, "a=@@B@@ b=2 @@X@@");
    }

    #[test]
    fn previous_trailing_slash_trimmed() {
        let s = render_hook("pre-commit", EXE, "C:/hooks/").unwrap();
        assert!(s.contains("PREV=\"C:/hooks\"\n"));
    }

    #[test]
    fn home_expansion() {
        assert_eq!(
            expand_home("~/.githooks", Some(Path::new(r"C:\Users\me"))),
            "C:/Users/me/.githooks"
        );
        assert_eq!(expand_home("C:/x", Some(Path::new("C:/h"))), "C:/x");
        assert_eq!(expand_home("~/x", None), "~/x");
    }

    #[test]
    fn same_dir_ignores_slashes_case_trailing() {
        assert!(same_dir(r"C:\Data\githop\hooks", "c:/data/githop/hooks/"));
        assert!(!same_dir("C:/data/githop/hooks", "C:/data/other"));
    }

    #[test]
    fn sha256_known_value() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn snippet_is_fail_open() {
        let s = snippet("commit", EXE).unwrap();
        assert_eq!(
            s,
            "\"C:/Users/Harshal/AppData/Local/Programs/GitHop/githop.exe\" check --stage commit -- \"$@\"; [ $? -ne 1 ] || exit 1"
        );
        assert!(snippet("nope", EXE).is_err());
    }

    #[test]
    fn health_ok_logic() {
        let mut h = HookHealth {
            installed: true,
            points_to_us: true,
            sh_path: Some(PathBuf::from("sh")),
            ..HookHealth::default()
        };
        assert!(h.is_ok());
        assert!(!h.needs_reinstall());
        h.tampered.push("pre-commit".into());
        assert!(!h.is_ok());
        assert!(h.needs_reinstall());
    }
}
