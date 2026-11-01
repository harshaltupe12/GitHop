//! Global git hooks: render, install, uninstall, verify (Phase 3 §3.1–3.2, §4.1;
//! Plan §7.7, §8 C2/C4).
//!
//! GitHop owns `<data dir>/hooks/` and points global `core.hooksPath` at it
//! (through its own include, read last, when an included file also sets
//! `core.hooksPath`; see [`install_hooks`] and ADR 0004).
//! Five hooks run a check (`pre-commit`, `pre-merge-commit`,
//! `prepare-commit-msg` for revert/cherry-pick, `pre-push`, `post-checkout`);
//! every other hook git knows is a pass-through stub, because a global
//! `core.hooksPath` would otherwise hide each repo's own `.git/hooks/` (C2).
//! Every script chains to `<git common dir>/hooks/<name>` and then to the
//! user's previous global hooks path (C4), running a hook file only once.
//!
//! Scripts are POSIX `sh` (run by Git for Windows' bundled `sh`), LF only, no
//! BOM. `githop.exe` is embedded as an absolute, quoted path, so hooks never
//! depend on `PATH`. Exit semantics inside the scripts: githop exit 1 → block
//! (githop also exits 1 for its own error under `block-strict`); 3 → continue
//! (githop already printed the warning); anything else, or no githop at all →
//! block when `settings.toml` says `enforce = "block-strict"`, else print the
//! fail-open warning and continue. The scripts read that one setting at run
//! time with shell built-ins, and only on that failure path, so the hooks
//! never go stale when the mode changes, by hand or not (regression D5).
//! `post-checkout` never blocks. With `GITHOP_DISABLE=1`, `pre-push` still
//! hands the ref lines to githop, which only records them (regression D4).
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

/// Hooks that run a GitHop check. `pre-merge-commit` runs the commit check for
/// merge commits (git runs it instead of `pre-commit`); `prepare-commit-msg`
/// runs it only for `git revert` / `git cherry-pick`, which run no `pre-commit`
/// (review 2026-11-01 #5).
pub const CHECKED_HOOKS: [&str; 5] = [
    "pre-commit",
    "pre-push",
    "post-checkout",
    "pre-merge-commit",
    "prepare-commit-msg",
];

/// Hooks that only chain. Spec §3.1 list, plus the server-side and git-p4 hooks
/// (a global `core.hooksPath` also applies to local bare repos receiving a push,
/// e.g. `file://` remotes, and to git-p4).
pub const PASSTHROUGH_HOOKS: [&str; 23] = [
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
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
const TPL_PRE_MERGE_COMMIT: &str = include_str!("templates/pre-merge-commit.sh");
const TPL_PREPARE_COMMIT_MSG: &str = include_str!("templates/prepare-commit-msg.sh");
const TPL_PRE_PUSH: &str = include_str!("templates/pre-push.sh");
const TPL_POST_CHECKOUT: &str = include_str!("templates/post-checkout.sh");
const TPL_PASSTHROUGH: &str = include_str!("templates/passthrough.sh");
const TPL_LOCATE: &str = include_str!("templates/locate.sh");
const TPL_DYNAMIC: &str = include_str!("templates/dynamic.sh");
const TPL_CHECK: &str = include_str!("templates/check.sh");
const TPL_CHAIN: &str = include_str!("templates/chain.sh");
const TPL_HELPERS: &str = include_str!("templates/helpers.sh");

/// How the check snippet calls githop for the commit-time hooks.
const INVOKE_COMMIT: &str = "  \"$GITHOP\" check --stage commit -- \"$@\"";
/// pre-push: feed the buffered ref lines when another hook also needs them.
const INVOKE_PUSH: &str = "  if [ -n \"$BUFFERED\" ]; then\n    \
     { [ -z \"$INPUT\" ] || printf '%s\\n' \"$INPUT\"; } | \"$GITHOP\" check --stage push -- \"$@\"\n  \
     else\n    \"$GITHOP\" check --stage push -- \"$@\"\n  fi";
/// `GITHOP_DISABLE=1`, commit-time hooks: nothing to do.
const DISABLED_COMMIT: &str = "  :";
/// `GITHOP_DISABLE=1`, pre-push: githop only records what the push publishes,
/// so Fix never rewrites it (regression D4). It prints nothing; its exit code
/// is ignored.
const DISABLED_PUSH: &str = "  if [ -f \"$GITHOP\" ]; then\n    \
     if [ -n \"$BUFFERED\" ]; then\n      \
     { [ -z \"$INPUT\" ] || printf '%s\\n' \"$INPUT\"; } | \"$GITHOP\" check --stage push -- \"$@\" >/dev/null 2>&1\n    \
     else\n      \"$GITHOP\" check --stage push -- \"$@\" >/dev/null 2>&1\n    fi\n  fi";

/// File (in GitHop's data dir) that holds GitHop's `core.hooksPath` when an
/// included git config file also sets one; the global config includes it last.
pub const INCLUDE_FILE: &str = "hooks.gitconfig";

/// Section header of the include line GitHop appends.
const OUR_INCLUDE_HEADER: &str = "[include]";

/// Most `[include]` / `[includeIf]` lines of the global config that are read.
const MAX_INCLUDES: usize = 32;

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
/// chaining a previous hooks folder (global, or set by an included file) that
/// has one of them.
fn wants_all_stubs(settings: &Settings, chained_dirs: &[String]) -> bool {
    settings.all_hook_stubs
        || chained_dirs.iter().any(|d| {
            !d.is_empty()
                && OPTIONAL_HOOKS
                    .iter()
                    .any(|h| Path::new(d).join(h).is_file())
        })
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

/// `/c/Users/me/hooks` (MSYS style, which Git for Windows' sh understands) →
/// `C:/Users/me/hooks`. Other paths are returned as they are.
pub fn msys_to_windows(path: &str) -> String {
    let b = path.as_bytes();
    if cfg!(windows)
        && b.len() >= 2
        && b[0] == b'/'
        && b[1].is_ascii_alphabetic()
        && (b.len() == 2 || b[2] == b'/')
    {
        format!(
            "{}:/{}",
            (b[1] as char).to_ascii_uppercase(),
            path.get(3..).unwrap_or("")
        )
    } else {
        path.to_string()
    }
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

/// Everything a rendered hook depends on besides its name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderOptions {
    /// Absolute path of `githop.exe` (any slash style).
    pub githop_exe: String,
    /// Previous global hooks path to chain to, already `~`-expanded; empty = none.
    pub previous: String,
    /// `settings.toml` (absolute). The scripts read `enforce` from it only
    /// when githop is missing or crashed, to decide block-strict at run time
    /// (regression D5): the hooks don't change with the enforce mode, so a
    /// mode changed by hand applies at once. Empty = never strict.
    pub settings_file: String,
    /// `Some(GitHop's hooks dir)` when an included git config file sets
    /// `core.hooksPath`: the scripts then pick the folder to chain to at run
    /// time (`git config --get-all core.hooksPath`, last value that isn't
    /// GitHop's), so an `includeIf` that applies only to some repos keeps
    /// working exactly there.
    pub dynamic_ours: Option<String>,
}

impl RenderOptions {
    /// Plain options: no strict mode, static chaining.
    pub fn new(githop_exe: &str, previous: &str) -> Self {
        Self {
            githop_exe: githop_exe.to_string(),
            previous: previous.to_string(),
            ..Self::default()
        }
    }
}

/// Render one hook script with default options (see [`render_hook_with`]).
pub fn render_hook(name: &str, githop_exe: &str, previous: &str) -> Result<String> {
    render_hook_with(name, &RenderOptions::new(githop_exe, previous))
}

/// Render one hook script. Pure. Output is LF-only, starts with `#!/bin/sh`,
/// and has no BOM.
pub fn render_hook_with(name: &str, opts: &RenderOptions) -> Result<String> {
    let (template, invoke, what) = match name {
        "pre-commit" => (TPL_PRE_COMMIT, INVOKE_COMMIT, "commit"),
        "pre-merge-commit" => (TPL_PRE_MERGE_COMMIT, INVOKE_COMMIT, "commit"),
        "prepare-commit-msg" => (TPL_PREPARE_COMMIT_MSG, INVOKE_COMMIT, "commit"),
        "pre-push" => (TPL_PRE_PUSH, INVOKE_PUSH, "push"),
        "post-checkout" => (TPL_POST_CHECKOUT, "", "clone"),
        n if all_hook_names().contains(&n) => (TPL_PASSTHROUGH, "", ""),
        other => return Err(invalid_input(format!("{other} is not a hook GitHop knows"))),
    };
    let githop_exe = opts.githop_exe.as_str();
    if CHECKED_HOOKS.contains(&name) && !paths::is_absolute_str(githop_exe) {
        return Err(invalid_input(format!(
            "githop path must be absolute, got {githop_exe}"
        )));
    }
    let exe = sh_quote_value(githop_exe)?;
    let prev = sh_quote_value(opts.previous.trim_end_matches(['/', '\\']))?;
    let buffer = if STDIN_HOOKS.contains(&name) {
        BUFFER_SNIPPET
    } else {
        ""
    };
    let dynamic = match &opts.dynamic_ours {
        Some(ours) => {
            let ours = sh_quote_value(ours.trim_end_matches(['/', '\\']))?;
            substitute(&lf(TPL_DYNAMIC), &[("OURS", &ours)])
        }
        None => String::new(),
    };
    let locate = substitute(&lf(TPL_LOCATE), &[("DYNAMIC", &dynamic)]);
    let disabled = if name == "pre-push" {
        DISABLED_PUSH
    } else {
        DISABLED_COMMIT
    };
    let helpers = lf(TPL_HELPERS);
    let check = format!(
        "{}{}",
        helpers,
        substitute(
            &lf(TPL_CHECK),
            &[("INVOKE", invoke), ("WHAT", what), ("DISABLED", disabled)],
        )
    );
    let chain = lf(TPL_CHAIN);
    let settings = sh_quote_value(&opts.settings_file)?;
    let rendered = substitute(
        &lf(template),
        &[
            ("GITHOP", &exe),
            ("PREVIOUS", &prev),
            ("SETTINGS", &settings),
            ("HELPERS", helpers.trim_end()),
            ("HOOK", name),
            ("LOCATE", locate.trim_end()),
            ("CHECK", check.trim_end()),
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
    render_names_with(names, &RenderOptions::new(githop_exe, previous))
}

/// Render the given hooks with `opts`: `(file name, contents)`, in order.
pub fn render_names_with(names: &[&str], opts: &RenderOptions) -> Result<Vec<(String, String)>> {
    names
        .iter()
        .map(|n| render_hook_with(n, opts).map(|c| (n.to_string(), c)))
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
    if stage == "push" {
        // pre-push gets the ref lines on stdin, and the rest of the repo's
        // hook (e.g. `git lfs pre-push`) needs them too (regression D6): read
        // them once, feed githop a copy, then give the hook's stdin back.
        return Ok(format!(
            "GITHOP_IN=$(cat); printf '%s\\n' \"$GITHOP_IN\" | \"{exe}\" check --stage push -- \"$@\"; \
             [ $? -ne 1 ] || exit 1; exec <<GITHOP_EOF\n$GITHOP_IN\nGITHOP_EOF"
        ));
    }
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

/// A `core.hooksPath` set by a file the global git config includes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncludedHooksPath {
    /// `includeIf` condition as written (e.g. `gitdir/i:C:/work/`); `None` for
    /// a plain `[include]` (or a value in the global file itself).
    pub condition: Option<String>,
    /// The file that sets it, as written in the include line.
    pub file: String,
    /// The value it sets (its last one).
    pub value: String,
}

impl IncludedHooksPath {
    /// "~/.gitconfig-work (for repos matching gitdir/i:C:/work/)".
    pub fn describe(&self) -> String {
        match &self.condition {
            Some(c) => format!("{} (for repos matching {c})", self.file),
            None => self.file.clone(),
        }
    }
}

/// How `core.hooksPath` is set in the global git config, includes included.
/// Read by [`scan_global`]; never written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalHooksLayout {
    /// The global config file.
    pub file: PathBuf,
    /// `core.hooksPath` values written in that file itself, in file order.
    pub own: Vec<String>,
    /// Included files (not GitHop's own) that set `core.hooksPath`, in file order.
    pub included: Vec<IncludedHooksPath>,
    /// The include line for GitHop's [`INCLUDE_FILE`] is there.
    pub our_include: bool,
    /// What the global file says without other includes: its last own value,
    /// or GitHop's dir when GitHop's include comes last.
    pub file_value: Option<String>,
    /// Included `core.hooksPath` values read *after* GitHop's, so they win over
    /// it where they apply (git uses the last value). Empty when GitHop's value
    /// is last, or absent. (An own value after GitHop's shows in `file_value`.)
    pub after_ours: Vec<IncludedHooksPath>,
}

impl GlobalHooksLayout {
    /// An included file sets `core.hooksPath`: GitHop then installs through
    /// its own include, read last (see [`install_hooks`]).
    pub fn include_mode(&self) -> bool {
        !self.included.is_empty()
    }
}

/// GitHop's include file (`<data dir>/hooks.gitconfig`).
pub fn include_file_path(paths: &Paths) -> PathBuf {
    paths.root().join(INCLUDE_FILE)
}

/// How the include line names [`include_file_path`] (forward slashes).
fn include_value(paths: &Paths) -> String {
    paths::to_slash(&include_file_path(paths))
}

/// Resolve an include path like git: `~/` from home, relative to the including file.
fn resolve_include(value: &str, including: &Path, home: Option<&Path>) -> PathBuf {
    let expanded = expand_home(value, home);
    if paths::is_absolute_str(&expanded) || expanded.starts_with("~/") {
        PathBuf::from(expanded)
    } else {
        including
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(expanded)
    }
}

/// Read how `core.hooksPath` is set in the global git config, following its
/// `[include]` and `[includeIf]` lines (review 2026-11-01 #1). Spawns: one for
/// the global file plus one per include line (at most 32). Conditional
/// includes are reported with their condition; they can't be evaluated here,
/// so a value behind any condition counts.
pub fn scan_global(git: &Git, paths: &Paths) -> Result<GlobalHooksLayout> {
    let ours = hooks_dir_value(paths);
    let our_file = include_value(paths);
    let file = git.global_config_path()?;
    let snapshot = gitconfig::list_file(git, &file)?;
    let home = git.home_dir();
    let shown = paths::to_slash(&file);

    let mut layout = GlobalHooksLayout {
        file: file.clone(),
        ..GlobalHooksLayout::default()
    };
    // (definition, is GitHop's, set in the global file itself), in file order.
    let mut defs: Vec<(IncludedHooksPath, bool, bool)> = Vec::new();
    let mut includes = 0usize;
    for e in snapshot.entries() {
        let key = e.key.as_str();
        let value = e.value.clone().unwrap_or_default();
        if gitconfig::key_eq(key, HOOKS_PATH_KEY) {
            if value.trim().is_empty() {
                continue;
            }
            layout.own.push(value.clone());
            let is_ours = same_dir(&value, &ours);
            let def = IncludedHooksPath {
                condition: None,
                file: shown.clone(),
                value,
            };
            defs.push((def, is_ours, true));
            continue;
        }
        let condition = if key.eq_ignore_ascii_case("include.path") {
            None
        } else if let Some(cond) = key
            .get(..10)
            .filter(|p| p.eq_ignore_ascii_case("includeif."))
            .and_then(|_| key[10..].strip_suffix(".path"))
        {
            Some(cond.to_string())
        } else {
            continue;
        };
        if value.trim().is_empty() {
            continue;
        }
        if same_dir(&expand_home(&value, home.as_deref()), &our_file) {
            layout.our_include = true;
            let def = IncludedHooksPath {
                condition: None,
                file: value,
                value: ours.clone(),
            };
            defs.push((def, true, false));
            continue;
        }
        includes += 1;
        if includes > MAX_INCLUDES {
            continue;
        }
        let target = resolve_include(&value, &file, home.as_deref());
        let set = gitconfig::get_all_file_includes(git, &target, HOOKS_PATH_KEY)?;
        let Some(v) = set.into_iter().rev().find(|v| !v.trim().is_empty()) else {
            continue;
        };
        let is_ours = same_dir(&expand_home(&v, home.as_deref()), &ours);
        let def = IncludedHooksPath {
            condition,
            file: value,
            value: v,
        };
        if !is_ours {
            layout.included.push(def.clone());
        }
        defs.push((def, is_ours, false));
    }

    // The global file's own say: last own value, or ours when GitHop's include
    // is the last of them.
    layout.file_value = defs
        .iter()
        .rev()
        .find(|(d, is_ours, own)| *own || (*is_ours && d.condition.is_none()))
        .map(|(d, _, _)| d.value.clone());
    // Included values read after GitHop's (own values after it already make
    // `file_value` someone else's).
    if let Some(last_ours) = defs.iter().rposition(|(_, is_ours, _)| *is_ours) {
        layout.after_ours = defs[last_ours + 1..]
            .iter()
            .filter(|(_, is_ours, own)| !is_ours && !own)
            .map(|(d, _, _)| d.clone())
            .collect();
    }
    Ok(layout)
}

/// What [`install_hooks`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReport {
    /// Value written to global `core.hooksPath`.
    pub hooks_dir: String,
    /// Previous global hooks path GitHop chains to (and restores on uninstall).
    pub previous_hooks_path: Option<String>,
    /// `core.hooksPath` values set by included git config files. GitHop's value
    /// is then set through its own include, read last, and the hooks chain to
    /// whichever of these applies in each repo.
    #[serde(default)]
    pub included_hooks_paths: Vec<IncludedHooksPath>,
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

/// Quote a value for a git config file (`"..."`, `\` and `"` escaped).
fn git_config_quote(value: &str) -> Result<String> {
    if let Some(c) = value.chars().find(|c| c.is_control()) {
        return Err(invalid_input(format!(
            "this path contains a control character ({c:?}), so GitHop can't put it in git config"
        )));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Contents of GitHop's include file.
fn include_file_contents(ours: &str) -> Result<String> {
    Ok(format!(
        "# Written by GitHop (githop hooks install). Your global git config includes\n\
         # this file last, so GitHop's hooks run even though another included file\n\
         # sets core.hooksPath; GitHop's hooks then run that folder's hooks too.\n\
         # githop hooks uninstall removes this file and the include line.\n\
         [core]\n\thooksPath = {}\n",
        git_config_quote(ours)?
    ))
}

/// Append GitHop's include line at the very end of the global config file, so
/// it's read after every other include. Atomic; follows a symlinked file.
fn append_our_include(file: &Path, paths: &Paths) -> Result<()> {
    let target = match std::fs::symlink_metadata(file) {
        Ok(m) if m.file_type().is_symlink() => {
            dunce::canonicalize(file).map_err(|e| CoreError::io(file, e))?
        }
        _ => file.to_path_buf(),
    };
    let mut bytes = match std::fs::read(&target) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(CoreError::io(&target, e)),
    };
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.extend_from_slice(if bytes.contains(&b'\r') {
            b"\r\n"
        } else {
            b"\n"
        });
    }
    let line = format!(
        "{OUR_INCLUDE_HEADER}\n\tpath = {}\n",
        git_config_quote(&include_value(paths))?
    );
    bytes.extend_from_slice(line.as_bytes());
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
    }
    crate::store::atomic_write(&target, &bytes)
}

/// Remove every include line that names GitHop's include file.
fn remove_our_include(git: &Git, paths: &Paths, file: &Path) -> Result<()> {
    let our_file = include_value(paths);
    let home = git.home_dir();
    let snapshot = gitconfig::list_file(git, file)?;
    let mut seen: Vec<String> = Vec::new();
    for e in snapshot.entries() {
        if !e.key.eq_ignore_ascii_case("include.path") {
            continue;
        }
        let Some(v) = e.value.as_deref() else {
            continue;
        };
        if same_dir(&expand_home(v, home.as_deref()), &our_file) && !seen.iter().any(|s| s == v) {
            gitconfig::unset_file_value(git, file, "include.path", v)?;
            seen.push(v.to_string());
        }
    }
    if !seen.is_empty() {
        trim_empty_trailing_include(file)?;
    }
    Ok(())
}

/// `git config --unset` leaves the section header behind. When the file now
/// ends with the empty `[include]` header GitHop appended, drop it, so an
/// install + uninstall leaves the file as it was.
fn trim_empty_trailing_include(file: &Path) -> Result<()> {
    let target = match std::fs::symlink_metadata(file) {
        Ok(m) if m.file_type().is_symlink() => {
            dunce::canonicalize(file).map_err(|e| CoreError::io(file, e))?
        }
        Ok(_) => file.to_path_buf(),
        Err(_) => return Ok(()),
    };
    let bytes = std::fs::read(&target).map_err(|e| CoreError::io(&target, e))?;
    let header = OUR_INCLUDE_HEADER.as_bytes();
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r' | b' ' | b'\t') {
        end -= 1;
    }
    let body = &bytes[..end];
    if !body.ends_with(header) {
        return Ok(());
    }
    let start = end - header.len();
    if start > 0 && bytes[start - 1] != b'\n' {
        return Ok(());
    }
    crate::store::atomic_write(&target, &bytes[..start])
}

/// Previous path, expanded, plus the folders included files set: every folder
/// a hook may chain to.
fn chained_dirs(previous: &str, layout: &GlobalHooksLayout, home: Option<&Path>) -> Vec<String> {
    let mut dirs = vec![expand_home(previous, home)];
    dirs.extend(layout.included.iter().map(|i| expand_home(&i.value, home)));
    dirs
}

/// Render options and file names for `settings` + `layout`.
fn plan_render(
    paths: &Paths,
    settings: &Settings,
    layout: &GlobalHooksLayout,
    exe: &str,
    previous: &str,
    home: Option<&Path>,
) -> (Vec<&'static str>, RenderOptions) {
    let opts = RenderOptions {
        githop_exe: exe.to_string(),
        previous: expand_home(previous, home),
        settings_file: paths::to_slash(&paths.settings_file()),
        dynamic_ours: layout.include_mode().then(|| hooks_dir_value(paths)),
    };
    let names = installed_hook_names(wants_all_stubs(
        settings,
        &chained_dirs(previous, layout, home),
    ));
    (names, opts)
}

/// What [`write_hook_files`] did.
struct Written {
    written: Vec<String>,
    removed: Vec<String>,
    /// File name → SHA-256 of what was written.
    hashes: BTreeMap<String, String>,
}

/// Write the rendered hook files atomically and remove old GitHop hook files
/// no longer generated.
fn write_hook_files(
    paths: &Paths,
    settings: &Settings,
    rendered: &[(String, String)],
) -> Result<Written> {
    let dir = paths.hooks_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let mut hashes = BTreeMap::new();
    let mut written = Vec::new();
    for (name, contents) in rendered {
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
    Ok(Written {
        written,
        removed,
        hashes,
    })
}

/// Install (or re-install) the hooks. See the module docs for what the caller
/// must do before (back up global git config) and after (save `settings`).
///
/// 1. Read how global `core.hooksPath` is set, includes included
///    ([`scan_global`]). The global file's own value, if set and not GitHop's,
///    becomes `settings.previous_hooks_path`. If it *is* GitHop's (re-install),
///    the recorded previous path is kept, so hooks never chain to themselves.
/// 2. Render every script (fails before touching anything if the exe path is
///    unusable).
/// 3. Write each file atomically; remove old GitHop hook files no longer generated.
/// 4. Point git at the hooks dir:
///    * **Usual case** (no included file sets `core.hooksPath`): set it in the
///      global file (`--replace-all`, so the user's value is replaced in place;
///      uninstall puts it back).
///    * **An included file sets `core.hooksPath`** (company hooks, often via
///      `includeIf`): git uses the *last* value, so a value in the global file
///      can lose to the include. GitHop then leaves the user's own value where
///      it is, writes its value to its own [`INCLUDE_FILE`] and appends an
///      `[include]` for it at the very end of the global file. Uninstall
///      removes just that include line, so the file is back as it was (git
///      may leave an empty `[include]` header). The scripts chain at run time
///      to the folder git would have used without GitHop in that repo.
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
    let layout = scan_global(git, paths)?;
    let ours_in_own = layout.own.iter().any(|v| same_dir(v, &ours));
    let own_user = layout
        .own
        .iter()
        .rev()
        .find(|v| !same_dir(v, &ours))
        .cloned();
    let reinstall = settings.hooks_installed || ours_in_own || layout.our_include;
    let previous = match own_user.as_deref() {
        Some(v) => v.to_string(),
        None if ours_in_own || layout.our_include => settings.previous_hooks_path.clone(),
        None => String::new(),
    };
    // Never chain to ourselves, whatever settings say.
    let previous = if !previous.is_empty() && same_dir(&previous, &ours) {
        String::new()
    } else {
        previous
    };
    let home = git.home_dir();
    let (names, opts) = plan_render(paths, settings, &layout, &exe, &previous, home.as_deref());
    let rendered = render_names_with(&names, &opts)?;
    let Written {
        written,
        removed,
        hashes,
    } = write_hook_files(paths, settings, &rendered)?;

    if layout.include_mode() {
        // GitHop's value must not stay in the global file itself (it was put
        // there by an install before the include appeared): put the user's
        // value back in its place, or drop it.
        if ours_in_own {
            let mut restored = own_user.is_some() || previous.is_empty();
            let mut seen: Vec<&str> = Vec::new();
            for v in layout.own.iter().filter(|v| same_dir(v, &ours)) {
                if seen.contains(&v.as_str()) {
                    continue;
                }
                seen.push(v);
                if restored {
                    gitconfig::unset_file_value(git, &layout.file, HOOKS_PATH_KEY, v)?;
                } else {
                    gitconfig::replace_file_value(git, &layout.file, HOOKS_PATH_KEY, v, &previous)?;
                    restored = true;
                }
            }
        }
        let inc = include_file_path(paths);
        if let Some(dir) = inc.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
        }
        let inc_text = include_file_contents(&ours)?;
        crate::store::atomic_write(&inc, inc_text.as_bytes())?;
        settings.hooks_include_hash = sha256_hex(inc_text.as_bytes());
        let ours_wins = layout
            .file_value
            .as_deref()
            .is_some_and(|v| same_dir(v, &ours));
        if !layout.our_include || !layout.after_ours.is_empty() || ours_in_own || !ours_wins {
            remove_our_include(git, paths, &layout.file)?;
            append_our_include(&layout.file, paths)?;
        }
    } else {
        if layout.our_include {
            remove_our_include(git, paths, &layout.file)?;
        }
        remove_include_file(paths)?;
        settings.hooks_include_hash.clear();
        gitconfig::set_global(git, HOOKS_PATH_KEY, &ours)?;
    }

    settings.previous_hooks_path = previous.clone();
    settings.hook_hashes = hashes;
    settings.hooks_installed = true;
    settings.githop_exe = exe;
    Ok(InstallReport {
        hooks_dir: ours,
        previous_hooks_path: (!previous.is_empty()).then_some(previous),
        included_hooks_paths: layout.included.clone(),
        written,
        removed,
        reinstall,
    })
}

fn remove_include_file(paths: &Paths) -> Result<()> {
    let inc = include_file_path(paths);
    match std::fs::remove_file(&inc) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::io(&inc, e)),
    }
}

/// Re-render the hook files when the hooks are installed (e.g. after an
/// upgrade or a stub setting changed; the enforce mode isn't baked in, so it
/// doesn't need this any more). Rewrites only GitHop's
/// hook files and `settings.hook_hashes`; never writes git config, so no
/// backup is needed. Returns `false` when there was nothing to refresh (not
/// installed, or no recorded `githop.exe`). Caller saves `settings`.
pub fn refresh_if_installed(paths: &Paths, git: &Git, settings: &mut Settings) -> Result<bool> {
    if !settings.hooks_installed || settings.githop_exe.is_empty() {
        return Ok(false);
    }
    let layout = scan_global(git, paths)?;
    let home = git.home_dir();
    let exe = settings.githop_exe.clone();
    let previous = settings.previous_hooks_path.clone();
    let (names, opts) = plan_render(paths, settings, &layout, &exe, &previous, home.as_deref());
    let rendered = render_names_with(&names, &opts)?;
    settings.hook_hashes = write_hook_files(paths, settings, &rendered)?.hashes;
    Ok(true)
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

/// Remove the hooks. GitHop's include line and [`INCLUDE_FILE`] are removed
/// (the user's own value, left in place at install, is then in effect again).
/// Global `core.hooksPath` in the file itself: if it still points at GitHop
/// (or is unset) → restore `settings.previous_hooks_path` exactly, or unset it
/// when there was none; if the user changed it to something else → leave it
/// alone. Deletes GitHop's hook files (and the dir when empty), then clears
/// `hooks_installed`, `hook_hashes` and `previous_hooks_path`.
/// Caller backs up global git config first and saves `settings` afterwards.
pub fn uninstall_hooks(
    paths: &Paths,
    git: &Git,
    settings: &mut Settings,
) -> Result<UninstallReport> {
    let ours = hooks_dir_value(paths);
    let layout = scan_global(git, paths)?;
    if layout.our_include {
        remove_our_include(git, paths, &layout.file)?;
    }
    remove_include_file(paths)?;
    let current = layout.own.last().cloned();
    let change = match current {
        Some(c) if !same_dir(&c, &ours) => {
            if layout.our_include && c == settings.previous_hooks_path {
                // Include mode left it in place: it's back in effect.
                HooksPathChange::Restored { value: c }
            } else {
                HooksPathChange::LeftAlone { value: c }
            }
        }
        Some(_) => {
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
        None if layout.our_include || settings.previous_hooks_path.is_empty() => {
            HooksPathChange::Unset
        }
        None => {
            gitconfig::set_global(git, HOOKS_PATH_KEY, &settings.previous_hooks_path)?;
            HooksPathChange::Restored {
                value: settings.previous_hooks_path.clone(),
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
    settings.hooks_include_hash.clear();
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
    /// Current global `core.hooksPath` as the global file itself sets it
    /// (GitHop's include counts), if set.
    pub global_hooks_path: Option<String>,
    /// Global `core.hooksPath` points at GitHop's dir.
    pub points_to_us: bool,
    /// `core.hooksPath` values git reads after GitHop's (from included files,
    /// often behind an `includeIf`): GitHop's hooks don't run where they apply.
    #[serde(default)]
    pub overridden_by: Vec<IncludedHooksPath>,
    /// `core.hooksPath` values set by included files that GitHop chains to.
    #[serde(default)]
    pub included_hooks_paths: Vec<IncludedHooksPath>,
    /// Hook files that should exist but don't.
    pub missing: Vec<String>,
    /// Files whose SHA-256 differs from the one recorded at install (edited by hand).
    pub tampered: Vec<String>,
    /// Files unchanged since install but not what this GitHop would write now
    /// (older templates, or a changed setting).
    pub stale: Vec<String>,
    /// Files in GitHop's hooks folder that GitHop didn't write (git runs any
    /// file there named like a hook, in every repo; security regression #5).
    #[serde(default)]
    pub unexpected: Vec<String>,
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
            && self.overridden_by.is_empty()
            && self.missing.is_empty()
            && self.tampered.is_empty()
            && self.stale.is_empty()
            && self.unexpected.is_empty()
            && !self.exe_missing
            && !self.previous_missing
            && self.sh_path.is_some()
            && self.config_error.is_none()
    }

    /// A re-install (`githop init` / "Repair") would fix what's wrong.
    pub fn needs_reinstall(&self) -> bool {
        self.installed
            && (!self.points_to_us
                || !self.overridden_by.is_empty()
                || !self.missing.is_empty()
                || !self.tampered.is_empty()
                || !self.stale.is_empty())
    }
}

/// Check the installed hooks. Never fails; problems are reported in the struct.
/// Spawns: [`scan_global`] (one, plus one per include line).
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
    let layout = match scan_global(git, paths) {
        Ok(layout) => {
            h.points_to_us = layout
                .file_value
                .as_deref()
                .is_some_and(|c| same_dir(c, &ours));
            h.global_hooks_path = layout.file_value.clone();
            h.overridden_by = layout.after_ours.clone();
            h.included_hooks_paths = layout.included.clone();
            layout
        }
        Err(e) => {
            h.config_error = Some(e.to_string());
            GlobalHooksLayout::default()
        }
    };
    h.exe_missing = settings.githop_exe.is_empty() || !Path::new(&settings.githop_exe).is_file();
    let home = git.home_dir();
    let prev_expanded =
        msys_to_windows(&expand_home(&settings.previous_hooks_path, home.as_deref()));
    h.previous_missing = !settings.previous_hooks_path.is_empty()
        && paths::is_absolute_str(&prev_expanded)
        && !Path::new(&prev_expanded).is_dir();

    if !settings.hooks_installed {
        return h;
    }
    let (names, opts) = plan_render(
        paths,
        settings,
        &layout,
        &settings.githop_exe,
        &settings.previous_hooks_path,
        home.as_deref(),
    );
    let expected: BTreeMap<String, String> = render_names_with(&names, &opts)
        .map(|v| {
            v.into_iter()
                .map(|(n, c)| (n, sha256_hex(c.as_bytes())))
                .collect()
        })
        .unwrap_or_default();
    let dir = paths.hooks_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut extra: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !names.contains(&n.as_str()) && !n.starts_with(".githop-hook-"))
            .collect();
        extra.sort();
        h.unexpected = extra;
    }
    // Include mode: GitHop's include file is read last, so anything added to
    // it beats the user's own global config (security regression #5).
    let inc = include_file_path(paths);
    if layout.include_mode() || layout.our_include || !settings.hooks_include_hash.is_empty() {
        match std::fs::read(&inc) {
            Ok(bytes) => {
                let expected = include_file_contents(&hooks_dir_value(paths))
                    .map(|t| sha256_hex(t.as_bytes()))
                    .unwrap_or_default();
                if sha256_hex(&bytes) != expected {
                    h.tampered.push(INCLUDE_FILE.to_string());
                }
            }
            Err(_) if layout.our_include => h.missing.push(INCLUDE_FILE.to_string()),
            Err(_) => {}
        }
    }
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

/// The `core.hooksPath` git uses in a repo when it isn't GitHop's hooks dir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoHooksOverride {
    /// The value in effect; `None` = no `core.hooksPath` at all.
    pub value: Option<String>,
    /// Where it comes from (`git config --show-origin`, `file:` stripped).
    pub origin: String,
    pub scope: crate::gitconfig::ConfigScope,
}

/// Effective `core.hooksPath` from a repo's config snapshot (which follows
/// includes and evaluates `includeIf` for that repo), when it is not GitHop's
/// hooks dir. Repo-local values (husky…, C3) are included; callers that
/// already warn about those can filter on `scope`. `None` when GitHop's hooks
/// run here, or when `snapshot` isn't from inside a repo. Pure.
pub fn repo_hooks_override(
    snapshot: &crate::gitconfig::ConfigSnapshot,
    paths: &Paths,
    home: Option<&Path>,
) -> Option<RepoHooksOverride> {
    use crate::gitconfig::ConfigScope;
    let in_repo = snapshot
        .entries()
        .iter()
        .any(|e| e.scope == ConfigScope::Local);
    if !in_repo {
        return None;
    }
    let ours = hooks_dir_value(paths);
    match snapshot.get_entry(HOOKS_PATH_KEY) {
        Some(e) => {
            let value = e.value.clone().unwrap_or_default();
            if same_dir(&expand_home(&value, home), &ours) {
                return None;
            }
            Some(RepoHooksOverride {
                value: Some(value),
                origin: e.origin.trim_start_matches("file:").to_string(),
                scope: e.scope,
            })
        }
        None => Some(RepoHooksOverride {
            value: None,
            origin: String::new(),
            scope: ConfigScope::Unknown,
        }),
    }
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
        assert_eq!(names.len(), 5 + 23);
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
            "echo \"$GW GitHop couldn't check this commit (it went through). Run: githop doctor\" >&2"
        ));
        assert!(s.contains("if [ \"$GITHOP_DISABLE\" = \"1\" ]; then\n  :\n"));
        // Design §9.3 wording for a missing githop too (regression D22).
        assert!(!s.contains("is not installed correctly (it went through)"));
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

    /// Review 2026-11-01 #5: merges and sequencer commits are checked.
    #[test]
    fn merge_and_sequencer_hooks_run_the_commit_check() {
        let m = render_hook("pre-merge-commit", EXE, "").unwrap();
        assert!(m.contains("\"$GITHOP\" check --stage commit -- \"$@\"\n"));
        assert!(m.contains("if [ $rc -eq 1 ]; then exit 1; fi"));
        let p = render_hook("prepare-commit-msg", EXE, "").unwrap();
        assert!(p.contains("\"$GITHOP\" check --stage commit -- \"$@\"\n"));
        // Only for revert/cherry-pick: `git commit` (incl. --no-verify, C15)
        // exports GIT_AUTHOR_EMAIL and is checked in pre-commit.
        assert!(p.contains("message) GITHOP_SEQ=1 ;;"));
        assert!(p.contains("if [ -n \"$GITHOP_SEQ\" ] && [ -z \"${GIT_AUTHOR_EMAIL+x}\" ]"));
        assert!(p.contains("rebase-merge"));
        assert!(render_hook("pre-merge-commit", "relative/githop.exe", "").is_err());
    }

    /// Regression D5: block-strict is read from settings.toml at run time,
    /// never baked in; D4: a disabled push still records.
    #[test]
    fn strict_mode_is_read_at_run_time() {
        let opts = RenderOptions {
            settings_file: r"C:\Users\a$b\githop\settings.toml".into(),
            ..RenderOptions::new(EXE, "")
        };
        let push = render_hook_with("pre-push", &opts).unwrap();
        assert!(push.contains("SETTINGS=\"C:/Users/a\\$b/githop/settings.toml\"\n"));
        assert!(push.contains("githop_strict()"));
        assert!(push.contains("enforce*block-strict*) return 0 ;;"));
        assert!(push.contains("so the push was blocked (block-strict mode)"));
        assert!(push.contains("| \"$GITHOP\" check --stage push -- \"$@\""));
        assert!(!push.contains("STRICT="));
        // Disabled: the push is still handed to githop, quietly.
        assert!(push.contains("check --stage push -- \"$@\" >/dev/null 2>&1"));
        let commit = render_hook_with("pre-commit", &opts).unwrap();
        assert!(commit.contains("if [ $rc -ne 0 ] && [ $rc -ne 3 ]; then"));
        assert!(!render_hook("commit-msg", EXE, "")
            .unwrap()
            .contains("githop_strict"));
        // ASCII symbols for the scripts' own lines (regression D13).
        assert!(commit.contains("GX=\"[blocked]\"; GW=\"[warn]\""));
        // Merge source too (security regression #3).
        let p = render_hook("prepare-commit-msg", EXE, "").unwrap();
        assert!(p.contains("merge) case \"$GIT_REFLOG_ACTION\" in merge*|pull*) ;;"));
        assert!(render_hook("pre-merge-commit", EXE, "")
            .unwrap()
            .contains("GITHOP_MERGE=1"));
        // Submodule checkouts run the clone check (regression D10).
        assert!(render_hook("post-checkout", EXE, "")
            .unwrap()
            .contains("*/.git/modules/*"));
    }

    /// Review 2026-11-01 #1/#8: chain target picked at run time only when an
    /// included file sets core.hooksPath; same hook file never runs twice.
    #[test]
    fn dynamic_chain_and_dedupe_snippets() {
        let s = render_hook("commit-msg", EXE, "C:/old").unwrap();
        assert!(!s.contains("git config --get-all core.hooksPath"));
        assert!(s.contains("[ \"$REPO_HOOK\" -ef \"$PREV_HOOK\" ]"));
        let d = render_hook_with(
            "commit-msg",
            &RenderOptions {
                dynamic_ours: Some("C:/Users/a$b/githop/hooks/".into()),
                ..RenderOptions::new(EXE, "C:/old")
            },
        )
        .unwrap();
        assert!(d.contains("$(git config --get-all core.hooksPath 2>/dev/null)"));
        assert!(d.contains("OURS=\"C:/Users/a\\$b/githop/hooks\"\n"));
        assert!(d.contains("PREV=\"C:/old\"\n"));
        let ours = d.find("OURS=").unwrap();
        let repo_hook = d.find("REPO_HOOK=\"\"").unwrap();
        assert!(
            ours < repo_hook,
            "PREV is final before the hooks are located"
        );
        assert!(!d.contains("@@"));
    }

    #[test]
    fn git_config_quoting() {
        assert_eq!(git_config_quote("C:/a b/x").unwrap(), "\"C:/a b/x\"");
        assert_eq!(git_config_quote(r#"C:\a"b"#).unwrap(), r#""C:\\a\"b""#);
        assert!(git_config_quote("C:/a\nb").is_err());
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

    /// Security regression 9.4: an MSYS-style previous hooks path exists.
    #[test]
    fn msys_paths_become_windows_paths() {
        if cfg!(windows) {
            assert_eq!(msys_to_windows("/c/Users/me/hooks"), "C:/Users/me/hooks");
            assert_eq!(msys_to_windows("/d"), "D:/");
        }
        assert_eq!(msys_to_windows("/usr/share"), "/usr/share");
        assert_eq!(msys_to_windows("C:/x"), "C:/x");
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
        // Regression D6: the push snippet hands stdin back to the hook.
        let p = snippet("push", EXE).unwrap();
        assert!(p.starts_with("GITHOP_IN=$(cat); printf '%s\\n' \"$GITHOP_IN\" | \""));
        assert!(p.contains("[ $? -ne 1 ] || exit 1; exec <<GITHOP_EOF\n$GITHOP_IN\nGITHOP_EOF"));
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
        let h = HookHealth {
            installed: true,
            points_to_us: true,
            sh_path: Some(PathBuf::from("sh")),
            overridden_by: vec![IncludedHooksPath {
                condition: Some("gitdir/i:C:/work/".into()),
                file: "~/.gitconfig-work".into(),
                value: "~/corp-hooks".into(),
            }],
            ..HookHealth::default()
        };
        assert!(!h.is_ok());
        assert!(h.needs_reinstall());
    }
}
