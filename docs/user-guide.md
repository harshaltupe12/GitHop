# GitHop user guide

GitHop keeps your git identities apart on one Windows PC. Each **profile**
(Office, Personal, …) has its own name, email and SSH key. GitHop picks the right
profile for every repo and **blocks** commits and pushes made with the wrong one,
before they leave your machine.

GitHop works offline. It never connects to the internet. No telemetry, no
accounts. The only network traffic is git and ssh doing your own pushes and
pulls, and `ssh -T git@github.com` when you ask GitHop to test a key.

> Screenshots: placeholders until the tray app ships. Each `[screenshot: …]`
> marks where one goes.

---

## Contents

1. [How it works](#1-how-it-works)
2. [Getting started](#2-getting-started)
3. [The three safety layers](#3-the-three-safety-layers)
4. [Command reference](#4-command-reference)
5. [Moving to another PC (export / import)](#5-moving-to-another-pc-export--import)
6. [Troubleshooting](#6-troubleshooting)
7. [Uninstall and wipe](#7-uninstall-and-wipe)
8. [Known limits](#8-known-limits)
9. [Where GitHop keeps things](#9-where-githop-keeps-things)

---

## 1. How it works

### Which profile does a repo use?

GitHop decides in this order and stops at the first match:

1. **Pin** — you (or GitHop on clone) fixed this repo to a profile. Stored in the
   repo's own `.git/config` as `githop.profile`.
2. **Owner** — the GitHub user or organisation in the repo's `origin` URL
   (`git@github.com:office-org/app.git` → owner `office-org`). Every profile lists
   the owners it covers. SSH host aliases that point at `github.com` work too.
3. **Folder** — the repo lives inside one of the profile's folders, e.g.
   `C:/Office/`. When folders nest, the longest match wins.
4. **Default profile** — used when nothing above matches. Switch it any time with
   `githop use <profile>` (or from the tray).

If the owner and the folder disagree, the owner wins and GitHop tells you once.
If nothing matches and there's no default profile, GitHop asks you to pin the repo:
`githop pin <profile>`.

### What GitHop changes on your PC

- A **backup** of your global git config is saved before GitHop changes anything.
  You can restore it any time with `githop wipe`.
- Global `core.hooksPath` points at GitHop's hooks folder. Your existing hooks
  keep running: GitHop's hooks call each repo's own `.git/hooks/<name>` and any
  global hooks path you had before.
- The default profile sets your global `user.name`, `user.email` and
  `core.sshCommand` (so ssh offers only that profile's key).
- Repos are pinned in their own `.git/config`.

GitHop never edits system-level git config, and never stores passwords, tokens
or key passphrases — only paths to your SSH keys.

`[screenshot: Profiles screen with Office and Personal]`

---

## 2. Getting started

```powershell
githop init                      # back up git config, create your first profile, turn on protection
githop add                       # add another profile (asks for each value)
githop keygen personal           # create an SSH key for it and print the public key
githop test personal             # check GitHub accepts the key (connects to github.com)
githop scan C:\Projects --apply  # find repos and pin each to its profile
githop status                    # inside a repo: which profile, and why
```

Tips:

- For a personal profile, use your GitHub **noreply** email
  (`12345+you@users.noreply.github.com`, from GitHub → Settings → Emails). Your
  real email then never appears in public commits.
- Give the key a passphrase (the default). Windows' OpenSSH agent can remember it
  for you — see [ssh-agent](#ssh-agent-keeps-asking-for-the-passphrase).
- Cloning a repo pins it automatically: `✓ GitHop: pinned to Personal`.

`[screenshot: onboarding — Safety first step]`

---

## 3. The three safety layers

| Layer | When | What happens |
|---|---|---|
| 1. Prevent | `git commit` | The `pre-commit` hook checks the author and committer email against the repo's profile. Wrong identity → **blocked**, with the command that fixes it. |
| 2. Safety net | `git push` | The `pre-push` hook checks every commit about to be pushed. Any commit with the wrong email → **blocked**. It also warns if the SSH key isn't the profile's key. |
| 3. Repair | `githop fix` | Re-stamps your **unpushed** commits with the right identity. A backup ref is saved first, so it can be undone. Pushed commits are never touched. |

Example — commit blocked:

```
✗ GitHop: commit blocked
This repo belongs to Office.
Commit would use me@personal.example.
Run: githop pin office
```

**What happens on a wrong identity** is a setting (`enforce` in
`settings.toml`, or Settings in the app):

| Mode | Wrong identity | GitHop itself has a problem |
|---|---|---|
| `block` (default) | blocked | warns, lets the commit through |
| `warn` | warns only | warns, lets the commit through |
| `block-strict` | blocked | blocked |

GitHop **fails open** by default: if GitHop itself breaks inside a hook, you see
`! GitHop couldn't check this commit (it went through). Run: githop doctor` and
your work isn't blocked by a GitHop bug.

`[screenshot: Fix screen with two commits to fix]`

---

## 4. Command reference

Every command accepts `--no-color`, `-q/--quiet` and `-v/--verbose`.
`--json` prints JSON for `list`, `status`, `scan`, `doctor` and `test`.
Colours are off when `NO_COLOR` is set; `GITHOP_ASCII=1` uses `[ok]`,
`[blocked]`, `[warn]` instead of symbols.

Exit codes: `0` ok · `1` blocked / problem found · `2` usage error (wrong
input, wrong password, bad file) · `3` internal error (hooks treat it as fail-open).

| Command | What it does |
|---|---|
| `githop init [--id --label --name --email --color --key] [-y]` | First-time setup: backs up your global git config, creates your first profile from your current identity, installs the hooks. |
| `githop add [--id --label --name --email --color --owner … --folder … --key]` | Add a profile. Asks for anything missing when run in a terminal. Default key path: `~/.ssh/githop_<id>`. |
| `githop edit <id> [--label --name --email --color --owner … / --no-owners --folder … / --no-folders --key --repin]` | Change a profile. |
| `githop remove <id> [-y] [--new-default <id>]` | Remove a profile. Its SSH key is kept. |
| `githop list` | All profiles; marks the default profile. |
| `githop use <id>` | Make a profile the default profile (global name, email and key). |
| `githop status` | In a repo: the profile it uses and why, the identity git will use, pin and warnings. |
| `githop pin <id>` / `githop unpin` | Pin this repo to a profile / remove the pin. |
| `githop scan <folder> [--apply [--include-default]] [--depth N]` | Find repos and show which profile owns each. `--apply` pins the ones matched by an owner or folder rule. |
| `githop keygen <id> [--no-passphrase]` | Create an ed25519 key for a profile and print the public key to add on GitHub. |
| `githop test <id>` | Run `ssh -T git@github.com` with the profile's key. The only command that connects to the internet, and only when you run it. |
| `githop doctor` | Check hooks, key files, ssh config, ssh-agent and per-repo overrides, with a fix for each problem. |
| `githop fix` | Re-stamp unpushed commits that used the wrong identity (backup ref first). |
| `githop export <file> [--include-keys] [-y]` | Save your profiles to a password-protected file. See [§5](#5-moving-to-another-pc-export--import). |
| `githop import <file> [--on-conflict skip\|replace\|rename] [-y]` | Add profiles from an export file. |
| `githop wipe [-y] [--keep-keys \| --delete-keys]` | Remove GitHop and restore your original git settings. See [§7](#7-uninstall-and-wipe). |
| `githop check --stage clone\|commit\|push` | Used by the hooks. You don't normally run it yourself. |

---

## 5. Moving to another PC (export / import)

There is no sync service. To move your profiles, export them to a file, copy the
file yourself (USB stick, your own cloud drive), and import it on the other PC.

### Export

```powershell
githop export C:\Users\me\Desktop\profiles.githop
githop export profiles.githop --include-keys     # also copy your SSH private keys
```

- You're asked for a password twice (input is hidden). Use at least 12
  characters — a few random words work well.
- The file is encrypted with [age](https://age-encryption.org) using your password.
  Without the password nobody can open it, **not even GitHop**. A lost password
  can't be recovered.
- What's in it: your profiles (name, email, colour, owners, folders), which
  profile is the default, and three settings (what happens on a wrong identity,
  activity log on/off, log retention). Key paths are stored as file names only
  (`githop_office`), never as full paths.
- What's never in it: backups, logs, pinned repo paths, hooks state, your previous
  hooks path.
- `--include-keys` adds the private and public key files. A key that has its own
  passphrase stays protected by it inside the file. Keep the file somewhere safe.

### Import

```powershell
githop import profiles.githop
```

GitHop shows what's in the file and every clash with your existing profiles
before changing anything:

```
This file has 2 profiles (exported 2026-10-31):
Id        Label     Email                              SSH key
office    Office    me@company.com                     included
personal  Personal  12345+me@users.noreply.github.com  not included
! Office (office) clashes with your profiles:
  • same ID as your profile office
  • same email as your profile office
What should GitHop do with Office? skip, replace or rename [skip]:
```

For each clashing profile you choose:

| Choice | Effect |
|---|---|
| `skip` | Keep yours, don't import this one (or its key). |
| `replace` | Replace your clashing profile(s) with the one from the file. If its SSH key differs from the one on this PC, your old key file is **renamed** to `githop_<id>.replaced-<time>`, never deleted. |
| `rename` | Import it under a new ID (`office-2` by default). Owners and folders that already belong to another profile are left out. Its key is saved as `githop_<new id>`. A profile with the same email can't be renamed — choose skip or replace. |

A colour that's already taken is swapped for a free one automatically.

Without a terminal (scripts), pass `--on-conflict skip|replace|rename` and `--yes`.
The password can come from the `GITHOP_EXPORT_PASSWORD` environment variable.

After importing:

- Key paths point at **this** PC's `%USERPROFILE%\.ssh`. Imported keys are
  restricted so only your Windows account can read them. GitHop never
  overwrites an existing key file unless you chose `replace`.
- If a key wasn't in the file, GitHop tells you where to put it, or run
  `githop keygen <id>` to make a new one (and add it on GitHub).
- Settings from the file are only copied onto a PC with no profiles yet, so an
  import never weakens an existing setup.
- To make the imported default profile active for git everywhere:
  `githop use <id>`.

GitHop checks every import file strictly: size limit (1 MiB), exact layout,
no unknown fields, valid profiles, and key names of the form `githop_<id>` only
— nothing in the file can write outside `~/.ssh`.

`[screenshot: Settings → Move to another PC]`
`[screenshot: Import dialog with conflict choices]`

---

## 6. Troubleshooting

Start with `githop doctor`. It lists each problem with the step that fixes it.

### A repo uses husky, lefthook or pre-commit (local `core.hooksPath`)

These tools set `core.hooksPath` inside the repo, which hides GitHop's global
hooks there. `githop status` and `githop scan` warn about it. Add this line near
the top of the repo's `pre-commit` hook (and the same with `--stage push` in
`pre-push`):

```sh
"C:/Users/you/AppData/Local/Programs/GitHop/githop.exe" check --stage commit -- "$@"; [ $? -ne 1 ] || exit 1
```

It only stops the commit on a confirmed wrong identity; if GitHop itself has a
problem the commit goes through. The tray app's "How to fix" dialog shows the
line with the right path for your PC. Pushes from other repos are still checked.

### ssh offers the wrong key, or a `Host github.com` block in `~/.ssh/config`

A `Host github.com` (or `Host *`) block with an `IdentityFile` line can make ssh
offer that key before the profile's key. `githop doctor` reports such blocks
with their line numbers. Fixes:

- Remove the `IdentityFile` line from that block, or
- Move it under an alias such as `Host github-personal` with
  `HostName github.com` — aliases don't affect `git@github.com` traffic, and
  GitHop recognises them as GitHub.

GitHop sets `core.sshCommand` with `-i <key> -o IdentitiesOnly=yes`, so ssh
offers only the profile's key, even when ssh-agent holds several.

### ssh-agent keeps asking for the passphrase

Windows' OpenSSH agent can remember your key passphrase. In an admin PowerShell:

```powershell
Get-Service ssh-agent | Set-Service -StartupType Automatic
Start-Service ssh-agent
ssh-add $env:USERPROFILE\.ssh\githop_personal
```

On an office PC you may not have admin rights. Then git asks for the passphrase
on each push, or use the agent that comes with Git for Windows.

### `git commit --no-verify`

`--no-verify` skips the `pre-commit` hook, so the commit isn't checked. The
`pre-push` check still runs when you push (unless you also push with
`--no-verify`). If a wrong commit got in, `githop fix` re-stamps it before push.

### Turning GitHop off for one command

```powershell
$env:GITHOP_DISABLE = "1"; git commit -m "…"; Remove-Item Env:GITHOP_DISABLE
```

```sh
GITHOP_DISABLE=1 git commit -m "…"
```

Only GitHop's check is skipped; your repo's own hooks still run.

### A commit was blocked but the identity looks right

Run `githop status` in the repo. Common causes: the repo is pinned to another
profile (`githop pin <profile>` to change it), the repo's own `.git/config`
sets `user.email`, or `GIT_AUTHOR_EMAIL` / `GIT_COMMITTER_EMAIL` are set in your
environment.

### "Open a new terminal to use githop"

The installer adds GitHop to your user PATH. Terminals opened before the
install don't see it yet.

### Export or import errors

| Message | What to do |
|---|---|
| The password is wrong, or the file was changed after it was exported | Check the password. If it's right, the file was damaged or changed — export it again. |
| This isn't a GitHop export file… | You picked another file, or it was cut short while copying. |
| Some profiles in the file clash with yours | Choose skip, replace or rename (`--on-conflict` without a terminal). |
| A key file already exists at … | GitHop won't overwrite keys. Use `replace`, or move the old key away first. |

---

## 7. Uninstall and wipe

```powershell
githop wipe                 # asks you to type WIPE
githop wipe --delete-keys   # also delete SSH keys GitHop created (~/.ssh/githop_*)
```

Wipe, in order: removes GitHop's hooks (restores the global hooks path you had
before, if any), restores the global git settings GitHop changed from the first
backup, optionally unpins the repos GitHop pinned, removes the start-with-Windows
entry, then deletes GitHop's data folder. SSH keys are kept unless you ask for
them to be deleted. If wipe is interrupted, run it again to finish.

Uninstalling the app from Windows Settings runs the same wipe (keeping your SSH
keys). Choose "keep my settings" in the uninstaller to keep your profiles but
still restore git and remove the hooks.

`[screenshot: Settings → Wipe GitHop confirmation]`

---

## 8. Known limits

GitHop is honest about what it can't do:

- **Git config is plain text.** Your name, email and key paths are readable by
  anyone with access to your Windows account — including office IT.
- **Office IT can see personal activity.** Endpoint agents, network logs and
  backups on a managed PC can see your personal pushes to GitHub. No app can hide
  this on a machine your employer manages.
- **Pushed commits are public.** The commit email is visible on GitHub. Use your
  GitHub noreply email for personal work.
- **Checks can be skipped.** `--no-verify` and tools that don't run git hooks
  skip the commit check; the push check covers normal `git push`.
- **Employer IP.** Personal work done on company hardware may be claimed by your
  employer. Check your employment contract and IT policy before mixing personal
  projects onto a work PC. GitHop keeps identities apart in git; it doesn't change
  who owns the work.
- **GitHub only (v1).** Owners are matched for GitHub URLs. Other hosts fall back
  to folder rules and the default profile. SSH keys only; HTTPS tokens aren't
  managed.
- **Lost export password.** There's no recovery — by design, nothing is stored
  online.

---

## 9. Where GitHop keeps things

| Data | Location |
|---|---|
| Profiles | `%APPDATA%\githop\profiles.toml` |
| Settings | `%APPDATA%\githop\settings.toml` |
| Hooks | `%APPDATA%\githop\hooks\` |
| Backups (git config, profiles) | `%APPDATA%\githop\backups\` |
| Activity log (off by default) | `%APPDATA%\githop\logs\` — never records code, file names, commit messages or repo URLs; deleted after 7 days |
| SSH keys | `%USERPROFILE%\.ssh\githop_<id>` |
| Per-repo pin | `<repo>\.git\config` (`githop.profile`) |

Set `GITHOP_HOME` to use another data folder (mainly for testing).
