# 6. HTTPS remotes, GitHub host keys and the Windows ssh-agent

Date: 2026-11-01 · Status: accepted (pending owner review)

## Context

Real use on the owner's PC: the personal repo `harshaltupe12/GitHop` was
pinned to Personal, but its remote was `https://github.com/…`. A pin sets
`core.sshCommand`, which HTTPS ignores, so `git push` signed in with the
account Git Credential Manager had saved (the Office account) and got a 403.
Two more things broke pushes from VS Code, whose Git runs without a terminal:
the first SSH connect stopped at "Are you sure you want to continue connecting
(yes/no)?", and a key with a passphrase asked for it on every push. VS Code
can answer neither. Git for Windows' bundled `ssh` doesn't use the Windows
`ssh-agent` service, so a passphrase "remembered" there was never used.

## Decisions

1. **HTTPS → SSH switching** (core `remote_ssh`). Only GitHub HTTPS remotes
   (`https://[user@]github.com/owner/repo[.git]`, also `www.` and `http://`)
   are touched; every other URL is left as it is. A remote that a
   `url.<base>.insteadOf` rule already rewrites to SSH counts as SSH. The
   switch runs `git remote set-url` (and `--push` for each GitHub HTTPS push
   URL) with argument arrays, to `git@github.com:owner/repo.git`. The old URL
   is saved first in `githop.saved-remote-url-<name>` (push URLs in
   `githop.saved-remote-pushurl-<name>`; names other than ASCII letters and
   digits are hex-escaped). **A password or token in the URL is never saved**
   (only the user name is kept, CLAUDE.md rule 8); switching removes it from
   `.git/config`. Undo restores a URL only while the remote still has the SSH
   URL GitHop set. The host is always `github.com`: the pinned
   `core.sshCommand` already names the profile's key, so an SSH alias isn't
   needed.
2. **Policy** `settings.remote_https_policy`: `ask` (default) / `auto` /
   `never`. It applies when GitHop pins a repo: `githop pin`, `scan --apply`,
   the clone hook, the app's Pin and Pin all. `auto` switches and says so;
   `ask` asks (the app shows a dialog with "Always do this", which sets
   `auto`; the CLI prompts on a terminal and prints `githop remote ssh`
   otherwise; the clone hook only prints the command); `never` only warns.
   `githop remote ssh [--all-in <folder>]`, `githop remote undo` and `githop
   remote policy` do it by hand.
3. **Pre-push warning, never a block.** When a resolved repo pushes to a
   GitHub HTTPS URL (pre-push `$2`, after git's rewriting) and the policy isn't
   `never`, the push check adds: "GitHop: this push uses HTTPS, so Windows signs
   in with the account it saved — not <profile>'s key." with the fix. No spawn;
   fail-open semantics are unchanged. `status` and `doctor` (in a repo) say
   "Uses HTTPS: <profile>'s SSH key isn't used".
4. **GitHub's host keys are compiled in and pre-trusted.** `ssh::GITHUB_KNOWN_HOSTS`
   holds GitHub's published ed25519, ecdsa-sha2-nistp256 and (2023) RSA keys
   ([GitHub's SSH key fingerprints](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints));
   a unit test checks each against the published SHA-256 fingerprint.
   `ensure_github_known_hosts` appends them to `~/.ssh/known_hosts` unless a
   `github.com` (or `[github.com]:22`) line already has one of the current
   keys — creating `~/.ssh` and the file when missing, keeping every line,
   copying the old file to `backups/known_hosts.<time>` first and writing
   atomically. Called on keygen, profile save with a key file, test
   connection, pin (CLI, app, clone hook) and `remote ssh`. No network: the
   keys are in the binary. Hashed (`|1|…`) lines can't be read without the
   salt, so they don't count; adding the plain lines next to them is harmless.
5. **`core.sshCommand` names Windows OpenSSH.** When
   `%SystemRoot%\System32\OpenSSH\ssh.exe` exists, pins and the default profile
   write `"C:/Windows/System32/OpenSSH/ssh.exe" -i "<key>" -o IdentitiesOnly=yes`
   (quoted, forward slashes, same escaping as the key); otherwise plain `ssh`.
   Terminal git and VS Code's Git read the same config and run it through
   Git's `sh`, and Windows OpenSSH is the ssh that talks to the Windows
   ssh-agent. `IdentitiesOnly=yes` with `-i` still uses the agent's copy of that
   key. `test_connection` uses the same program. An integration test runs the
   pinned value through Git's `sh` with `-G` (prints the resolved config, never
   connects) and `-V`. Older pins keep plain `ssh` until they're re-pinned;
   "Remember my passphrase" re-pins the profile's repos (and rewrites the global
   value, backed up first, when it's the default profile).
6. **"Remember my passphrase"** (app key panel). GitHop reads only the key
   file's header to tell whether it has a passphrase, and asks the agent which
   keys it holds (`ssh-add -l`, SHA-256 fingerprints). With the `ssh-agent`
   service running, it starts Windows OpenSSH's `ssh-add <key>` in a **visible
   console window**; the passphrase is typed there and never passes through
   GitHop. With the service stopped or disabled, "Turn on and remember" runs
   one elevated `cmd.exe` through PowerShell's `Start-Process -Verb RunAs`
   (ShellExecute "runas", one UAC prompt) that runs `sc.exe config ssh-agent
   start= auto` and `sc.exe start ssh-agent`, then `ssh-add`. The workspace
   denies `unsafe` code, so this goes through PowerShell instead of calling
   `ShellExecuteExW` directly; the script is a compile-time constant with no
   user input in it. Everything that touches the system sits behind the
   `ssh::AgentHost` trait; tests use a stub and never start `sc`, `ssh-add` or
   an elevated process.

## Consequences

- Switching changes only the remote address; history, branches and the pin
  are untouched, and undo puts the URL back (without a token).
- A switched repo needs the profile's public key on the right GitHub account,
  which Test connection checks.
- `githop.saved-remote-*` keys are repo-local config; `wipe` leaves the
  switched URLs as they are (they keep working without GitHop).
