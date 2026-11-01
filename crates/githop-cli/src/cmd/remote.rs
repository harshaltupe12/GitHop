//! `githop remote ssh [--all-in <folder>]`, `githop remote undo`,
//! `githop remote policy [ask|auto|never]` — GitHub HTTPS remotes → SSH (ADR 0006).
//!
//! Over HTTPS, git signs in with the account Windows saved, not the profile's
//! SSH key. Switching the remote to `git@github.com:owner/repo.git` makes the
//! pinned key do the push. Nothing here connects anywhere.

use anyhow::Result;
use githop_core::remote_ssh::{self, HttpsAction, HttpsRemote};
use githop_core::{resolve, scan, ssh, RemoteHttpsPolicy, Repo, Store};

use super::{cwd, label_for, Ctx};
use crate::cli::{RemoteCommand, RemotePolicyArgs, RemoteSshArgs};
use crate::exit::{usage, Exit};

pub fn run(ctx: &Ctx, command: RemoteCommand) -> Result<Exit> {
    match command {
        RemoteCommand::Ssh(a) => run_ssh(ctx, a),
        RemoteCommand::Undo => run_undo(ctx),
        RemoteCommand::Policy(a) => run_policy(ctx, a),
    }
}

/// The profile label a repo resolves to, if any.
fn repo_label(ctx: &Ctx, store: &Store, repo: &Repo) -> Result<(Option<String>, bool)> {
    let git = ctx.git()?;
    let snapshot = repo.config_snapshot()?;
    let aliases = githop_core::aliases::current(store.paths(), git, store.settings());
    let res = resolve(&snapshot, repo.root(), store.profiles(), &aliases);
    let label = res
        .profile_id
        .as_deref()
        .filter(|id| store.profiles().get(id).is_some())
        .map(|id| label_for(store.profiles(), id));
    Ok((label, res.pinned.is_some()))
}

fn print_switched(ctx: &Ctx, list: &[HttpsRemote], label: Option<&str>) {
    for r in list {
        ctx.ui
            .ok(&format!("Switched {} to SSH: {}", r.name, r.ssh_url));
    }
    if let Some(l) = label {
        ctx.ui.hint(&format!("Pushes now use {l}'s SSH key."));
    }
}

fn run_ssh(ctx: &Ctx, args: RemoteSshArgs) -> Result<Exit> {
    ctx.reject_json("remote ssh")?;
    let git = ctx.git()?;
    let store = ctx.store()?;
    if let Some(folder) = args.all_in {
        if !folder.is_dir() {
            return Err(usage(format!(
                "There's no folder at {}.",
                githop_core::paths::to_slash(&folder)
            )));
        }
        let mut switched = 0usize;
        for dir in scan::find_repos(&folder, scan::DEFAULT_MAX_DEPTH) {
            let Ok(repo) = Repo::discover(git, &dir) else {
                continue;
            };
            let Ok((label, _)) = repo_label(ctx, &store, &repo) else {
                continue;
            };
            // Only repos with a profile: SSH needs to know whose key to use.
            if label.is_none() {
                continue;
            }
            match remote_ssh::switch_all_to_ssh(&repo) {
                Ok(list) if !list.is_empty() => {
                    let name = githop_core::paths::to_slash(repo.root());
                    for r in &list {
                        ctx.ui.ok(&format!("{name}: switched {} to SSH", r.name));
                    }
                    switched += 1;
                }
                Ok(_) => {}
                Err(e) => ctx.ui.warn(&format!(
                    "{}: couldn't switch ({e})",
                    githop_core::paths::to_slash(repo.root())
                )),
            }
        }
        if switched == 0 {
            ctx.ui
                .line("No repo with a profile uses a GitHub HTTPS remote here.");
        } else {
            ssh::ensure_github_known_hosts_best_effort(git, store.paths());
            ctx.ui.hint("Undo in a repo: githop remote undo");
        }
        return Ok(Exit::Ok);
    }

    let repo = Repo::discover(git, &cwd()?)?;
    let list = remote_ssh::switch_all_to_ssh(&repo)?;
    if list.is_empty() {
        ctx.ui
            .line("This repo has no GitHub HTTPS remote, so there's nothing to switch.");
        return Ok(Exit::Ok);
    }
    ssh::ensure_github_known_hosts_best_effort(git, store.paths());
    let (label, pinned) = repo_label(ctx, &store, &repo)?;
    print_switched(ctx, &list, label.as_deref().filter(|_| pinned));
    if !pinned {
        ctx.ui
            .warn("This repo isn't pinned, so SSH uses your default profile's key here.");
        ctx.ui
            .fix_hint("Pin it to the right profile: githop pin <profile>");
    }
    ctx.ui.hint("Undo: githop remote undo");
    Ok(Exit::Ok)
}

fn run_undo(ctx: &Ctx) -> Result<Exit> {
    ctx.reject_json("remote undo")?;
    let git = ctx.git()?;
    let repo = Repo::discover(git, &cwd()?)?;
    let restored = remote_ssh::restore_all(&repo)?;
    if restored.is_empty() {
        ctx.ui
            .line("GitHop hasn't switched a remote in this repo, so there's nothing to undo.");
    } else {
        for name in restored {
            ctx.ui.ok(&format!("{name} uses its HTTPS URL again"));
        }
    }
    Ok(Exit::Ok)
}

fn run_policy(ctx: &Ctx, args: RemotePolicyArgs) -> Result<Exit> {
    ctx.reject_json("remote policy")?;
    let mut store = ctx.store()?;
    let explain = |p: RemoteHttpsPolicy| match p {
        RemoteHttpsPolicy::Ask => "ask before switching a GitHub HTTPS remote to SSH",
        RemoteHttpsPolicy::Auto => "switch GitHub HTTPS remotes to SSH when pinning",
        RemoteHttpsPolicy::Never => "leave HTTPS remotes alone (only warn)",
    };
    match args.value {
        None => {
            let p = store.settings().remote_https_policy;
            ctx.ui.line(&format!("{}: {}", p.as_str(), explain(p)));
            ctx.ui
                .hint("Change it: githop remote policy ask|auto|never");
        }
        Some(v) => {
            let p = RemoteHttpsPolicy::parse(&v)
                .ok_or_else(|| usage("Pick one of: ask, auto, never"))?;
            store.update_settings(|s| {
                s.remote_https_policy = p;
                Ok(())
            })?;
            ctx.ui
                .ok(&format!("When pinning, GitHop will {}.", explain(p)));
        }
    }
    Ok(Exit::Ok)
}

/// After a pin (`githop pin`, `scan --apply`): apply the HTTPS policy and
/// trust GitHub's host keys. Never fails the pin; problems become warnings.
/// `ask` prompts on a terminal; without one it prints the command.
pub fn after_pin(ctx: &Ctx, store: &Store, repo: &Repo, label: &str, ask: bool) {
    if let Ok(git) = ctx.git() {
        ssh::ensure_github_known_hosts_best_effort(git, store.paths());
    }
    let policy = store.settings().remote_https_policy;
    let action = match remote_ssh::apply_policy(repo, policy) {
        Ok(a) => a,
        Err(e) => {
            ctx.ui
                .warn(&format!("GitHop couldn't switch this repo to SSH: {e}"));
            ctx.ui.fix_hint("Run: githop remote ssh");
            return;
        }
    };
    match action {
        HttpsAction::Nothing => {}
        HttpsAction::Switched(list) => {
            print_switched(ctx, &list, Some(label));
            ctx.ui.hint("Undo: githop remote undo");
        }
        HttpsAction::Warn(_) => {
            ctx.ui.warn(&remote_ssh::https_problem(label));
            ctx.ui.fix_hint("Run: githop remote ssh");
        }
        HttpsAction::Ask(list) => {
            ctx.ui.warn(&remote_ssh::https_problem(label));
            if !(ask && ctx.ui.interactive) {
                ctx.ui.fix_hint("Run: githop remote ssh");
                return;
            }
            let question = format!(
                "Switch {} to SSH ({}) so pushes use {label}'s key?",
                remote_ssh::names_and(&list),
                list.first().map(|r| r.ssh_url.as_str()).unwrap_or_default()
            );
            match ctx.ui.confirm(&question, true) {
                Ok(true) => match remote_ssh::switch_all_to_ssh(repo) {
                    Ok(done) => {
                        print_switched(ctx, &done, Some(label));
                        ctx.ui
                            .hint("To always do this when pinning: githop remote policy auto");
                    }
                    Err(e) => ctx.ui.warn(&format!("Couldn't switch: {e}")),
                },
                _ => ctx.ui.hint("Later: githop remote ssh"),
            }
        }
    }
}

/// `githop status` lines for a repo whose GitHub remote is HTTPS.
pub fn status_lines(ctx: &Ctx, https: &[HttpsRemote], label: &str) {
    if https.is_empty() {
        return;
    }
    ctx.ui.warn(&remote_ssh::https_problem(label));
    ctx.ui.fix_hint(&format!(
        "Run: githop remote ssh  ({})",
        remote_ssh::names_and(https)
    ));
}
