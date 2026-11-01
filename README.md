# GitHop

Hop between git identities on one Windows PC — safely, locally, automatically.

GitHop gives each identity (Office, Personal, …) its own name, email and SSH key,
picks the right one for every repo from its owner or folder, and **blocks commits
and pushes made with the wrong identity** before they leave your machine. A
command-line tool (`githop.exe`) does the work; a small tray app is planned on top.

## Quick start (CLI)

Put `githop.exe` somewhere permanent (the hooks call it by its full path), then:

```powershell
# 1. Set up: backs up your global git config, turns your current identity into
#    the first profile, and installs the hooks. Safe to run again.
githop init --id office

# 2. Add your other identity. Owners are the GitHub users/orgs whose repos use it.
githop add --id personal --name "Harshal" --email "12345+harshal@users.noreply.github.com" --owner harshal-personal
githop edit office --owner office-org

# 3. Give each profile its own SSH key, then add the printed public key on
#    GitHub (Settings → SSH and GPG keys) for that account.
githop keygen personal
githop test personal          # connects to github.com: "Connected as harshal-personal"

# 4. Pick the default profile: used for repos with no owner or folder match.
githop use office

# 5. Pin the repos you already have (each gets its own name, email and key).
githop scan C:\Code --apply
```

Check any repo with `githop status`, and your setup with `githop doctor`.
New clones of an owner's repos are pinned automatically.

## How it works

For every repo GitHop decides which profile owns it, in this order:

1. **Pin** — the repo was pinned (`githop pin <id>`, `scan --apply`, or automatically on clone).
2. **Owner** — the GitHub user/org in the `origin` URL (SSH host aliases for github.com work too).
3. **Folder** — the longest matching folder rule of a profile.
4. **Default profile** — what `githop use` set.

Pinning writes `user.name`, `user.email` and `core.sshCommand` (with
`IdentitiesOnly=yes`) into the repo's own `.git/config`, so plain `git` uses the
right identity without GitHop running. GitHop's hooks are installed by pointing
global `core.hooksPath` at `%APPDATA%\githop\hooks`; every hook still runs your
repo's own `.git/hooks/*` and any hooks folder you had before.

## Safety layers

| Layer | When | What happens |
|---|---|---|
| Auto-pin | `git clone` | The new repo is pinned to its owner's profile: `✓ GitHop: pinned to Personal` |
| Block commit | `git commit` | A commit that would use another profile's email is **blocked**. If the repo wasn't pinned yet, GitHop pins it and asks you to run the commit again. |
| Block push | `git push` | A push containing any commit with the wrong author or committer email is **blocked**, listing those commits. |
| Fix | `githop fix` | Re-stamps unpushed wrong-identity commits (never touches pushed ones, always keeps a backup ref). |

If GitHop itself has a problem (bad settings file, crash), git **keeps working**:
you see `! GitHop couldn't check this commit (it went through). Run: githop doctor`.
Set `enforce = "block-strict"` in `settings.toml` to block instead. `GITHOP_DISABLE=1`
turns the checks off for one command, like `--no-verify` for GitHop only.

Repos that set their own `core.hooksPath` (husky, lefthook, pre-commit) skip
global hooks. `githop doctor` and `githop status` point them out; add the line
from `githop hooks snippet commit` to the top of that repo's pre-commit hook.

## Known limits

- **Your identity is visible on this machine.** Git needs names, emails and key
  paths in plain-text config files; anyone with access to your Windows account,
  including office IT, can see them.
- **Office IT can see personal pushes.** Endpoint agents, proxies, network logs
  and backups on a managed PC can record traffic to GitHub. No app can hide that.
- **Pushed commits are public.** The commit email is visible to anyone who can
  see the repo — use a GitHub noreply address for personal profiles.
- **Checks can be skipped.** `git commit --no-verify`, `git push --no-verify`,
  `GITHOP_DISABLE=1`, and tools that don't run git hooks (some libgit2-based
  GUIs) bypass GitHop. The push check still catches commits made with
  `--no-verify` unless the push skips hooks too.
- **Employer IP.** Personal work done on company hardware or during work hours
  may be claimed by your employer. Check your employment contract and IT policy.

## Uninstall

```powershell
githop wipe
```

Wipe removes GitHop's hooks (restoring your previous `core.hooksPath`), puts back
the global `user.name`, `user.email` and `core.sshCommand` values from the backup
taken before GitHop changed anything (other settings you changed since are kept),
optionally unpins the repos GitHop pinned, removes the startup entry, and deletes
`%APPDATA%\githop`. Your SSH keys are kept unless you add `--delete-keys`. If wipe
is interrupted, run it again — it picks up where it stopped. JSON output for every
command that has it is described in [docs/cli-json.md](docs/cli-json.md).

## Privacy promise

- **No network code.** GitHop never makes a network request. The only traffic is
  git/ssh doing your own push/pull, and `ssh -T` when you ask to test a key.
  `cargo deny` bans HTTP/WebSocket client crates from the dependency tree.
- **No telemetry, analytics, crash reporting or auto-updater.**
- **No secrets stored.** Only paths to SSH keys — never passwords, tokens or passphrases.
- Everything lives in `%APPDATA%\githop` and your git config. Every global change
  is backed up first and can be undone with `githop wipe`.

## Legal note

Personal work done on an employer's hardware or network may be owned or monitored by
the employer. Check your employment contract and IT policy. GitHop keeps identities
apart in git; it can't hide anything from endpoint monitoring.

## Build

Requires Rust (MSVC toolchain on Windows) and Git for Windows ≥ 2.40.

```bash
cargo build                                 # build workspace
cargo test                                  # tests run in an isolated git env
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo deny check                            # licences, advisories, banned network crates
```

## Layout

```
crates/githop-core/   library: profiles, store, resolver, git config, hooks, checks
crates/githop-cli/    githop.exe — CLI and git hook entry point
app/                  Tauri tray app (later)
docs/decisions/       architecture decision records
```
