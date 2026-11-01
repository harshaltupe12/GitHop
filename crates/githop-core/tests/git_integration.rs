//! Integration tests for the git integration layer: repo discovery, config
//! snapshot, local/global writes, pin/unpin and effective identity.
//!
//! Every git process runs in the isolated `TestEnv` (CLAUDE.md rule 3).

// clippy's `allow-unwrap-in-tests` covers `#[test]` fns only, not helpers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};

use githop_core::gitconfig::{self, ConfigScope, ConfigSnapshot};
use githop_core::identity::{effective_identity, effective_identity_with};
use githop_core::pin::{self, PIN_KEY};
use githop_core::{CoreError, Profile, ProfileColor, Repo};
use support::TestEnv;

fn canon(p: &Path) -> PathBuf {
    dunce::canonicalize(p).unwrap()
}

fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn profile(id: &str, name: &str, email: &str, key: &str) -> Profile {
    Profile {
        id: id.into(),
        label: id.into(),
        name: name.into(),
        email: email.into(),
        color: ProfileColor::Blue,
        ssh_key: PathBuf::from(key),
        owners: vec![],
        folders: vec![],
    }
}

/// Escape bytes for readable test output.
fn show(raw: &[u8]) -> String {
    raw.iter()
        .map(|b| match b {
            0 => "\\0".to_string(),
            b'\n' => "\\n".to_string(),
            32..=126 => (*b as char).to_string(),
            _ => format!("\\x{b:02x}"),
        })
        .collect()
}

// ---------------------------------------------------------------- snapshot

/// Pins down the exact byte layout git 2.51 emits for
/// `git config --list --show-origin --show-scope -z`.
#[test]
fn snapshot_raw_byte_format() {
    let env = TestEnv::new();
    std::fs::write(env.home().join("inc.cfg"), "[foo]\n\tbar = 1\n").unwrap();
    std::fs::write(
        env.global_config(),
        "[user]\n\tname = Jörg Ünï\n[core]\n\tflag\n[alias]\n\tx = \"!echo a=b\\nline2\"\n\
         [include]\n\tpath = inc.cfg\n",
    )
    .unwrap();
    let repo = env.init_repo("raw");
    let out = env.git_output(
        &[
            "-c",
            "cmd.key=v",
            "config",
            "--list",
            "--show-origin",
            "--show-scope",
            "-z",
        ],
        &repo,
    );
    assert!(out.status.success());
    let raw = out.stdout;
    println!("RAW: {}", show(&raw));
    // Origins are printed verbatim: the global path exactly as GIT_CONFIG_GLOBAL
    // spells it (backslashes here), local as the relative `.git/config`.
    let gc = env.global_config().to_string_lossy().into_owned();
    let has = |needle: &[u8]| raw.windows(needle.len()).any(|w| w == needle);
    assert!(has(
        format!("global\0file:{gc}\0user.name\nJörg Ünï\0").as_bytes()
    ));
    assert!(has(format!("global\0file:{gc}\0core.flag\0").as_bytes()));
    assert!(has(format!(
        "global\0file:{gc}\0alias.x\n!echo a=b\nline2\0"
    )
    .as_bytes()));
    assert!(has(b"local\0file:.git/config\0core.bare\nfalse\0"));
    assert!(has(b"command\0command line:\0cmd.key\nv\0"));
    assert!(raw.ends_with(b"\0"));

    let s = ConfigSnapshot::parse(&raw).unwrap();
    assert_eq!(s.get("user.name"), Some("Jörg Ünï"));
    assert_eq!(s.get_entry("core.flag").unwrap().value, None);
    assert_eq!(s.get("alias.x"), Some("!echo a=b\nline2"));
    let inc = s.get_entry("foo.bar").unwrap();
    assert_eq!(inc.scope, ConfigScope::Global);
    assert!(inc.origin.ends_with("inc.cfg"), "{}", inc.origin);
    assert_eq!(s.get_entry("cmd.key").unwrap().scope, ConfigScope::Command);
}

#[test]
fn snapshot_scopes_origins_and_remote() {
    let env = TestEnv::new();
    env.set_global("user.email", "global@example.invalid");
    let repo = env.init_repo("scopes");
    env.git(
        &["config", "--local", "user.email", "local@example.invalid"],
        &repo,
    );
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:Office-Org/billing.git",
        ],
        &repo,
    );
    env.git(
        &["remote", "add", "upstream", "https://github.com/up/billing"],
        &repo,
    );
    let r = Repo::discover(&env.git_handle(), &repo).unwrap();
    let s = r.config_snapshot().unwrap();
    assert_eq!(s.get("user.email"), Some("local@example.invalid"));
    assert_eq!(
        s.get_scoped("user.email", ConfigScope::Global),
        Some("global@example.invalid")
    );
    let g = s
        .entries()
        .iter()
        .find(|e| e.scope == ConfigScope::Global && e.key == "user.email")
        .unwrap();
    assert_eq!(
        g.origin,
        format!("file:{}", env.global_config().to_string_lossy())
    );
    let l = s.get_entry("user.email").unwrap();
    assert_eq!(l.scope, ConfigScope::Local);
    assert!(l.origin.ends_with(".git/config"), "{}", l.origin);
    assert_eq!(
        s.remote_url("origin"),
        Some("git@github.com:Office-Org/billing.git")
    );
    assert_eq!(
        s.remotes(),
        vec!["origin".to_string(), "upstream".to_string()]
    );
    // no system scope: GIT_CONFIG_NOSYSTEM=1
    assert!(s.entries().iter().all(|e| e.scope != ConfigScope::System));
}

#[test]
fn snapshot_outside_repo_lists_global_only() {
    let env = TestEnv::new();
    env.set_global("user.name", "Outside");
    let s = ConfigSnapshot::read(&env.git_handle(), &env.home()).unwrap();
    assert_eq!(s.get("user.name"), Some("Outside"));
    assert!(s.entries().iter().all(|e| e.scope != ConfigScope::Local));
}

#[test]
fn snapshot_with_empty_config_is_empty() {
    let env = TestEnv::new();
    let s = ConfigSnapshot::read(&env.git_handle(), &env.home()).unwrap();
    assert!(s.entries().is_empty(), "{:?}", s.entries());
}

#[test]
fn snapshot_with_missing_global_file() {
    let env = TestEnv::new();
    std::fs::remove_file(env.global_config()).unwrap();
    let s = ConfigSnapshot::read(&env.git_handle(), &env.home()).unwrap();
    assert!(s.entries().is_empty());
}

// ---------------------------------------------------------------- discover

#[test]
fn discover_root_and_subdir() {
    let env = TestEnv::new();
    let repo = env.init_repo("disc");
    let sub = repo.join("src").join("deep dir");
    std::fs::create_dir_all(&sub).unwrap();
    let git = env.git_handle();
    for dir in [&repo, &sub] {
        let r = Repo::discover(&git, dir).unwrap();
        assert_eq!(r.root(), canon(&repo), "from {}", dir.display());
        assert_eq!(
            r.common_dir(),
            canon(&repo.join(".git")),
            "from {}",
            dir.display()
        );
        assert!(!r.root().to_string_lossy().starts_with(r"\\?\"));
    }
}

#[test]
fn discover_unicode_path() {
    let env = TestEnv::new();
    let repo = env.init_repo("Jörg 日本 repo");
    let r = Repo::discover(&env.git_handle(), &repo).unwrap();
    assert_eq!(r.root(), canon(&repo));
}

#[test]
fn discover_non_repo_is_not_a_repo() {
    let env = TestEnv::new();
    let err = Repo::discover(&env.git_handle(), &env.home()).unwrap_err();
    assert!(matches!(err, CoreError::NotARepo), "{err:?}");
}

#[test]
fn discover_bare_and_git_dir_are_not_a_repo() {
    let env = TestEnv::new();
    let bare = env.bare_remote("owner/bare");
    let err = Repo::discover(&env.git_handle(), &bare).unwrap_err();
    assert!(matches!(err, CoreError::NotARepo), "{err:?}");
    let repo = env.init_repo("inner");
    let err = Repo::discover(&env.git_handle(), &repo.join(".git")).unwrap_err();
    assert!(matches!(err, CoreError::NotARepo), "{err:?}");
}

#[test]
fn discover_missing_dir_is_io_error() {
    let env = TestEnv::new();
    let err = Repo::discover(&env.git_handle(), &env.root().join("nope")).unwrap_err();
    assert!(matches!(err, CoreError::Io { .. }), "{err:?}");
}

#[test]
fn discover_worktree_uses_main_common_dir() {
    let env = TestEnv::new();
    let main = env.init_repo("main");
    env.commit_as(&main, "a.txt", "T", "t@example.invalid");
    let wt = env.repos().join("wt");
    env.git(&["worktree", "add", "-q", "-b", "wt", &fwd(&wt)], &main);
    let r = Repo::discover(&env.git_handle(), &wt).unwrap();
    assert_eq!(r.root(), canon(&wt));
    assert_eq!(r.common_dir(), canon(&main.join(".git")));
    // a pin in the worktree lands in the shared (common) config
    let p = profile("work", "W", "w@example.invalid", "C:/k/id");
    pin::pin_repo(&r, &p).unwrap();
    let main_repo = Repo::discover(&env.git_handle(), &main).unwrap();
    assert_eq!(
        main_repo.config_snapshot().unwrap().get(PIN_KEY),
        Some("work")
    );
}

// ---------------------------------------------------------------- writes

#[test]
fn set_and_unset_local() {
    let env = TestEnv::new();
    let repo = env.init_repo("local");
    let git = env.git_handle();
    // duplicates are collapsed by --replace-all
    env.git(&["config", "--local", "--add", "x.dup", "1"], &repo);
    env.git(&["config", "--local", "--add", "x.dup", "2"], &repo);
    gitconfig::set_local(&git, &repo, "x.dup", "3").unwrap();
    assert_eq!(
        env.git(&["config", "--local", "--get-all", "x.dup"], &repo),
        "3"
    );
    // value with spaces, quotes and unicode survives
    gitconfig::set_local(&git, &repo, "x.v", r#"a "b" ü 日本 $HOME"#).unwrap();
    let s = ConfigSnapshot::read(&git, &repo).unwrap();
    assert_eq!(s.get("x.v"), Some(r#"a "b" ü 日本 $HOME"#));
    gitconfig::unset_local(&git, &repo, "x.dup").unwrap();
    // missing key is fine
    gitconfig::unset_local(&git, &repo, "x.dup").unwrap();
    gitconfig::unset_local(&git, &repo, "never.set").unwrap();
    let out = env.git_output(&["config", "--local", "--get", "x.dup"], &repo);
    assert_eq!(out.status.code(), Some(1));
    // the global config was not touched
    assert_eq!(std::fs::read_to_string(env.global_config()).unwrap(), "");
}

#[test]
fn global_get_set_unset() {
    let env = TestEnv::new();
    let git = env.git_handle();
    assert_eq!(gitconfig::get_global(&git, "user.email").unwrap(), None);
    gitconfig::unset_global(&git, "user.email").unwrap();
    gitconfig::set_global(&git, "user.email", "g@example.invalid").unwrap();
    assert_eq!(
        gitconfig::get_global(&git, "user.email")
            .unwrap()
            .as_deref(),
        Some("g@example.invalid")
    );
    gitconfig::unset_global(&git, "user.email").unwrap();
    assert_eq!(gitconfig::get_global(&git, "user.email").unwrap(), None);
    gitconfig::unset_global(&git, "user.email").unwrap();
}

#[test]
fn global_unset_with_missing_file_is_ok() {
    let env = TestEnv::new();
    std::fs::remove_file(env.global_config()).unwrap();
    let git = env.git_handle();
    gitconfig::unset_global(&git, "user.email").unwrap();
    assert_eq!(gitconfig::get_global(&git, "user.email").unwrap(), None);
}

#[test]
fn set_default_profile_global_writes_isolated_config_only() {
    let env = TestEnv::new();
    let real = dirs::home_dir().map(|h| h.join(".gitconfig"));
    let before = real
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());

    let p = profile(
        "personal",
        "Jane Ünï",
        "jane@users.noreply.github.com",
        r"C:\Users\Jane Doe\.ssh\id_personal",
    );
    pin::set_default_profile_global(&env.git_handle(), &p).unwrap();

    let text = std::fs::read_to_string(env.global_config()).unwrap();
    assert!(text.contains("jane@users.noreply.github.com"), "{text}");
    let git = env.git_handle();
    assert_eq!(
        gitconfig::get_global(&git, "user.name").unwrap().as_deref(),
        Some("Jane Ünï")
    );
    assert_eq!(
        gitconfig::get_global(&git, "core.sshCommand")
            .unwrap()
            .as_deref(),
        Some(r#"ssh -i "C:/Users/Jane Doe/.ssh/id_personal" -o IdentitiesOnly=yes"#)
    );
    assert_eq!(gitconfig::get_global(&git, PIN_KEY).unwrap(), None);

    let after = real
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    assert_eq!(before, after, "real ~/.gitconfig must not change");
}

// ---------------------------------------------------------------- pin

#[test]
fn pin_unpin_round_trip_keeps_user_keys() {
    let env = TestEnv::new();
    let repo = env.init_repo("pinme");
    env.git(&["config", "--local", "user.signingkey", "ABC123"], &repo);
    env.git(&["config", "--local", "core.autocrlf", "input"], &repo);
    let r = Repo::discover(&env.git_handle(), &repo).unwrap();
    let p = profile(
        "office",
        "Jane Office",
        "jane@corp.example",
        r"C:\Users\Jane Doe\.ssh\id_office",
    );

    pin::pin_repo(&r, &p).unwrap();
    let s = r.config_snapshot().unwrap();
    assert_eq!(s.get_scoped(PIN_KEY, ConfigScope::Local), Some("office"));
    assert_eq!(
        s.get_scoped("user.name", ConfigScope::Local),
        Some("Jane Office")
    );
    assert_eq!(
        s.get_scoped("user.email", ConfigScope::Local),
        Some("jane@corp.example")
    );
    let ssh = s.get_scoped("core.sshCommand", ConfigScope::Local).unwrap();
    assert_eq!(ssh, pin::ssh_command_for(&p.ssh_key));
    assert_eq!(
        pin::key_from_ssh_command(ssh).as_deref(),
        Some("C:/Users/Jane Doe/.ssh/id_office")
    );
    // the pin shows up as the effective identity
    let id = r.effective_identity().unwrap().unwrap();
    assert_eq!(id.author.email, "jane@corp.example");

    // re-pinning (e.g. to another profile) replaces cleanly
    let p2 = profile(
        "personal",
        "Jane",
        "jane@users.noreply.github.com",
        "C:/k/p",
    );
    pin::pin_repo(&r, &p2).unwrap();
    let s = r.config_snapshot().unwrap();
    assert_eq!(
        s.get_all("user.email"),
        vec!["jane@users.noreply.github.com"]
    );

    assert!(pin::unpin_repo(&r).unwrap());
    let s = r.config_snapshot().unwrap();
    for key in [PIN_KEY, "user.name", "user.email", "core.sshCommand"] {
        assert_eq!(s.get_scoped(key, ConfigScope::Local), None, "{key}");
    }
    assert_eq!(s.get("user.signingkey"), Some("ABC123"));
    assert_eq!(s.get("core.autocrlf"), Some("input"));
    // nothing left to unpin
    assert!(!pin::unpin_repo(&r).unwrap());
    // global untouched throughout
    assert_eq!(std::fs::read_to_string(env.global_config()).unwrap(), "");
}

#[test]
fn unpin_without_pin_leaves_user_identity() {
    let env = TestEnv::new();
    let repo = env.init_repo("mine");
    env.git(
        &["config", "--local", "user.email", "own@example.invalid"],
        &repo,
    );
    let r = Repo::discover(&env.git_handle(), &repo).unwrap();
    assert!(!pin::unpin_repo(&r).unwrap());
    assert_eq!(
        r.config_snapshot().unwrap().get("user.email"),
        Some("own@example.invalid")
    );
}

// ---------------------------------------------------------------- identity

#[test]
fn identity_from_global() {
    let env = TestEnv::new();
    env.set_global("user.name", "Glöbal Person 日本");
    env.set_global("user.email", "global@example.invalid");
    let repo = env.init_repo("id-global");
    let id = effective_identity(&env.git_handle(), &repo)
        .unwrap()
        .unwrap();
    assert_eq!(id.author.name, "Glöbal Person 日本");
    assert_eq!(id.author.email, "global@example.invalid");
    assert_eq!(id.committer, id.author);
}

#[test]
fn identity_local_overrides_global() {
    let env = TestEnv::new();
    env.set_global("user.name", "G");
    env.set_global("user.email", "global@example.invalid");
    let repo = env.init_repo("id-local");
    env.git(
        &["config", "--local", "user.email", "local@example.invalid"],
        &repo,
    );
    let r = Repo::discover(&env.git_handle(), &repo).unwrap();
    let id = r.effective_identity().unwrap().unwrap();
    assert_eq!(id.author.email, "local@example.invalid");
    assert_eq!(id.committer.email, "local@example.invalid");
}

#[test]
fn identity_respects_include_if_gitdir() {
    let env = TestEnv::new();
    let work = env.init_repo("work-repo");
    let other = env.init_repo("other-repo");
    std::fs::write(
        env.home().join("work.inc"),
        "[user]\n\temail = work@corp.example\n",
    )
    .unwrap();
    let gitdir = format!("{}/", fwd(&canon(&work)));
    std::fs::write(
        env.global_config(),
        format!(
            "[user]\n\tname = P\n\temail = personal@example.invalid\n\
             [includeIf \"gitdir:{gitdir}\"]\n\tpath = work.inc\n"
        ),
    )
    .unwrap();
    let git = env.git_handle();
    let id = effective_identity(&git, &work).unwrap().unwrap();
    assert_eq!(id.author.email, "work@corp.example");
    let id = effective_identity(&git, &other).unwrap().unwrap();
    assert_eq!(id.author.email, "personal@example.invalid");
    // the snapshot sees the include too, with its origin
    let s = ConfigSnapshot::read(&git, &work).unwrap();
    let e = s.get_entry("user.email").unwrap();
    assert!(e.origin.ends_with("work.inc"), "{}", e.origin);
}

#[test]
fn identity_env_override_is_caught() {
    let env = TestEnv::new();
    env.set_global("user.name", "G");
    env.set_global("user.email", "global@example.invalid");
    let repo = env.init_repo("id-env");
    let git = env
        .git_handle()
        .env("GIT_AUTHOR_EMAIL", "sneaky@example.invalid")
        .env("GIT_COMMITTER_NAME", "Env Committer");
    let id = effective_identity(&git, &repo).unwrap().unwrap();
    assert_eq!(id.author.email, "sneaky@example.invalid");
    assert_eq!(id.author.name, "G");
    assert_eq!(id.committer.email, "global@example.invalid");
    assert_eq!(id.committer.name, "Env Committer");
}

#[test]
fn identity_unset_with_use_config_only_is_none() {
    let env = TestEnv::new();
    env.set_global("user.useConfigOnly", "true");
    let repo = env.init_repo("id-none");
    let git = env.git_handle();
    // Note: `git var -l` itself exits 0 here and prints a non-strict
    // auto-detected ident (`unknown <user@HOST.(none)>`); the strict re-check
    // in `effective_identity` is what turns it into `None`.
    assert_eq!(effective_identity(&git, &repo).unwrap(), None);
    // git commit agrees: it refuses
    std::fs::write(repo.join("f"), "f").unwrap();
    env.git(&["add", "f"], &repo);
    let out = env.git_output(&["commit", "-q", "-m", "x"], &repo);
    assert!(!out.status.success());

    // name only → still none
    env.set_global("user.name", "N");
    assert_eq!(effective_identity(&git, &repo).unwrap(), None);
    // env email makes it valid, for the author only
    let with_env = git.clone().env("GIT_AUTHOR_EMAIL", "a@example.invalid");
    assert_eq!(effective_identity(&with_env, &repo).unwrap(), None);
    let both = with_env.env("GIT_COMMITTER_EMAIL", "c@example.invalid");
    let id = effective_identity(&both, &repo).unwrap().unwrap();
    assert_eq!(id.author.email, "a@example.invalid");
    assert_eq!(id.committer.email, "c@example.invalid");
    // configured email → valid
    env.set_global("user.email", "n@example.invalid");
    let id = effective_identity(&git, &repo).unwrap().unwrap();
    assert_eq!(id.author.email, "n@example.invalid");
}

/// `EMAIL` gives `git var -l` a plausible (non-`(none)`) email, but with
/// `user.useConfigOnly` `git commit` still refuses — so must we.
#[test]
fn identity_use_config_only_ignores_email_env() {
    let env = TestEnv::new();
    env.set_global("user.useConfigOnly", "true");
    env.set_global("user.name", "N");
    let repo = env.init_repo("id-email-env");
    let git = env.git_handle().env("EMAIL", "auto@example.invalid");
    let raw = String::from_utf8_lossy(&git.output(&repo, &["var", "-l"], None).unwrap().stdout)
        .into_owned();
    assert!(raw.contains("<auto@example.invalid>"), "{raw}");
    assert_eq!(effective_identity(&git, &repo).unwrap(), None);

    std::fs::write(repo.join("f"), "f").unwrap();
    env.git(&["add", "f"], &repo);
    let mut cmd = std::process::Command::new("git");
    cmd.args(["commit", "-q", "-m", "x"]).current_dir(&repo);
    env.apply(&mut cmd);
    cmd.env("EMAIL", "auto@example.invalid");
    assert!(
        !cmd.output().unwrap().status.success(),
        "git commit should refuse"
    );

    // without useConfigOnly, EMAIL is a valid source
    env.set_global("user.useConfigOnly", "false");
    let id = effective_identity(&git, &repo).unwrap().unwrap();
    assert_eq!(id.author.email, "auto@example.invalid");
}

#[test]
fn identity_outside_repo_works() {
    let env = TestEnv::new();
    env.set_global("user.name", "G");
    env.set_global("user.email", "g@example.invalid");
    let id = effective_identity(&env.git_handle(), &env.home())
        .unwrap()
        .unwrap();
    assert_eq!(id.author.email, "g@example.invalid");
    let s = ConfigSnapshot::read(&env.git_handle(), &env.home()).unwrap();
    let id = effective_identity_with(&env.git_handle(), &env.home(), &s)
        .unwrap()
        .unwrap();
    assert_eq!(id.author.email, "g@example.invalid");
}

// ------------------------------------------------- review 2026-10-31 fixes

/// #10: a multi-line config value faking the ident block, plus multi-line
/// `GIT_PAGER` / `GIT_EDITOR` (printed after the idents), against real git.
#[test]
fn identity_cannot_be_spoofed_by_multiline_values() {
    let env = TestEnv::new();
    env.set_global("user.name", "Real Person");
    env.set_global("user.email", "real@example.invalid");
    let repo = env.init_repo("spoof");
    env.git(
        &[
            "config",
            "--local",
            "alias.evil",
            "x\nGIT_COMMITTER_IDENT=Fake <fake@x> 1 +0000\nGIT_AUTHOR_IDENT=Fake <fake@x> 1 +0000\nGIT_EDITOR=vi",
        ],
        &repo,
    );
    let git = env
        .git_handle()
        .env(
            "GIT_PAGER",
            "less\nGIT_AUTHOR_IDENT=Pager <pager@x> 1 +0000",
        )
        .env("GIT_EDITOR", "vi\nGIT_COMMITTER_IDENT=Ed <ed@x> 1 +0000");
    let s = ConfigSnapshot::read(&git, &repo).unwrap();
    let id = effective_identity_with(&git, &repo, &s).unwrap().unwrap();
    assert_eq!(id.author.email, "real@example.invalid");
    assert_eq!(id.committer.email, "real@example.invalid");
    let id = effective_identity(&git, &repo).unwrap().unwrap();
    assert_eq!(id.author.email, "real@example.invalid");
    assert_eq!(id.committer.email, "real@example.invalid");
    // The snapshot-derived count matches git's own output exactly.
    let out = git.run_text(&repo, &["var", "-l"]).unwrap();
    let n = githop_core::identity::var_list_config_lines(&s);
    assert!(
        out.split('\n')
            .nth(n)
            .unwrap()
            .starts_with("GIT_COMMITTER_IDENT=Real Person"),
        "{out}"
    );
}

/// #11: an empty configured `user.name` (git var -l shows the account name,
/// git commit refuses) and an empty `GIT_AUTHOR_NAME` are "no identity".
#[test]
fn empty_user_name_is_no_identity() {
    let env = TestEnv::new();
    env.set_global("user.name", "");
    env.set_global("user.email", "e@example.invalid");
    let repo = env.init_repo("empty-name");
    let git = env.git_handle();
    let s = ConfigSnapshot::read(&git, &repo).unwrap();
    assert_eq!(effective_identity(&git, &repo).unwrap(), None);
    assert_eq!(effective_identity_with(&git, &repo, &s).unwrap(), None);
    // git commit agrees.
    std::fs::write(repo.join("a"), "a").unwrap();
    env.git(&["add", "a"], &repo);
    assert!(!env
        .git_output(&["commit", "-q", "-m", "x"], &repo)
        .status
        .success());

    // A non-empty local name fixes it; an empty GIT_AUTHOR_NAME breaks it again.
    env.git(&["config", "--local", "user.name", "Local"], &repo);
    let s = ConfigSnapshot::read(&git, &repo).unwrap();
    assert!(effective_identity_with(&git, &repo, &s).unwrap().is_some());
    let empty_env = git.clone().env("GIT_AUTHOR_NAME", "");
    let s = ConfigSnapshot::read(&empty_env, &repo).unwrap();
    assert_eq!(
        effective_identity_with(&empty_env, &repo, &s).unwrap(),
        None
    );
}

/// #3: pinning keeps the user's own local identity and unpinning restores it.
#[test]
fn pin_saves_and_unpin_restores_user_values() {
    let env = TestEnv::new();
    let repo = env.init_repo("own");
    env.git(&["config", "--local", "user.name", "Own Name"], &repo);
    env.git(
        &["config", "--local", "user.email", "own@example.invalid"],
        &repo,
    );
    env.git(
        &["config", "--local", "core.sshCommand", "ssh -i C:/own/key"],
        &repo,
    );
    let r = Repo::discover(&env.git_handle(), &repo).unwrap();
    let office = profile("office", "Office", "office@example.invalid", "C:/k/office");
    pin::pin_repo(&r, &office).unwrap();
    let s = r.config_snapshot().unwrap();
    assert_eq!(s.get("user.email"), Some("office@example.invalid"));
    assert_eq!(s.get("githop.saved-name"), Some("Own Name"));
    assert_eq!(s.get("githop.saved-email"), Some("own@example.invalid"));
    assert_eq!(s.get("githop.saved-sshcommand"), Some("ssh -i C:/own/key"));

    // Re-pinning to another profile must not save GitHop's own values.
    let personal = profile("personal", "Me", "me@example.invalid", "C:/k/me");
    pin::pin_repo(&r, &personal).unwrap();
    let s = r.config_snapshot().unwrap();
    assert_eq!(s.get("user.email"), Some("me@example.invalid"));
    assert_eq!(s.get("githop.saved-email"), Some("own@example.invalid"));

    assert!(pin::unpin_repo(&r).unwrap());
    let s = r.config_snapshot().unwrap();
    assert_eq!(
        s.get_scoped("user.name", ConfigScope::Local),
        Some("Own Name")
    );
    assert_eq!(
        s.get_scoped("user.email", ConfigScope::Local),
        Some("own@example.invalid")
    );
    assert_eq!(
        s.get_scoped("core.sshCommand", ConfigScope::Local),
        Some("ssh -i C:/own/key")
    );
    for key in [
        PIN_KEY,
        "githop.saved-name",
        "githop.saved-email",
        "githop.saved-sshcommand",
    ] {
        assert_eq!(s.get(key), None, "{key}");
    }

    // A value equal to the profile's is kept too (unpin never drops it).
    env.git(
        &["config", "--local", "user.email", "office@example.invalid"],
        &repo,
    );
    pin::pin_repo(&r, &office).unwrap();
    assert!(pin::unpin_repo(&r).unwrap());
    assert_eq!(
        r.config_snapshot()
            .unwrap()
            .get_scoped("user.email", ConfigScope::Local),
        Some("office@example.invalid")
    );
}

/// #8: the repo is derived from the hook environment without a spawn.
#[test]
fn repo_from_hook_env_needs_no_spawn() {
    let env = TestEnv::new();
    let main = env.init_repo("main");
    env.commit_as(&main, "a.txt", "T", "t@example.invalid");
    let wt = env.repos().join("wt");
    env.git(&["worktree", "add", "-q", "-b", "wt", &fwd(&wt)], &main);
    let git = env.git_handle();

    let r = Repo::from_hook_env(&git, &main).unwrap();
    assert_eq!(r.root(), canon(&main));
    assert_eq!(r.common_dir(), canon(&main.join(".git")));

    let r = Repo::from_hook_env(&git, &wt).unwrap();
    assert_eq!(r.root(), canon(&wt));
    assert_eq!(r.common_dir(), canon(&main.join(".git")));

    let gitdir = env.git(&["rev-parse", "--absolute-git-dir"], &wt);
    let with_env = git.clone().env("GIT_DIR", &gitdir);
    let r = Repo::from_hook_env(&with_env, &wt).unwrap();
    assert_eq!(r.root(), canon(&wt));
    assert_eq!(r.common_dir(), canon(&main.join(".git")));
    assert_eq!(git.spawn_count(), 0, "no spawn so far");

    // Same answers as `discover`.
    let d = Repo::discover(&git, &wt).unwrap();
    assert_eq!(d.root(), canon(&wt));
    assert_eq!(d.common_dir(), canon(&main.join(".git")));

    // A subdirectory (not a hook cwd) falls back to one `rev-parse`.
    let sub = main.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let before = git.spawn_count();
    let r = Repo::from_hook_env(&git, &sub).unwrap();
    assert_eq!(r.root(), canon(&main));
    assert_eq!(git.spawn_count() - before, 1);
    // Not a repo at all → NotARepo, like discover.
    assert!(matches!(
        Repo::from_hook_env(&git, &env.home()),
        Err(CoreError::NotARepo)
    ));
}
