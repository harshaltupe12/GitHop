# Changelog

All notable changes to GitHop are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and GitHop uses
[Semantic Versioning](https://semver.org/).

## [0.1.0] — Unreleased

First release. Windows only, GitHub only, SSH keys only.

### Profiles and storage
- Any number of profiles, each with its own label, name, email, colour, SSH key,
  GitHub owners and folders.
- `profiles.toml` and `settings.toml` in `%APPDATA%\githop`, written atomically
  under a file lock, with the last 5 versions of `profiles.toml` kept as backups.
- Validation that reports every problem at once (ids, emails, owners, folders,
  colours), plus a nudge to use the GitHub noreply email for personal profiles.

### Picking the right profile
- Resolution order: pin → owner (from the `origin` URL, incl. ssh host aliases
  for github.com) → longest folder rule → default profile.
- Clones are pinned automatically to their profile.
- `githop scan` finds repos in a folder and pins them in one go.

### HTTPS remotes, host keys and passphrases (ADR 0006)
- GitHub HTTPS remotes can be switched to SSH (`githop remote ssh`, app "Switch
  to SSH" / "Switch all to SSH"), with undo; a token in the old URL is never kept.
- `remote_https_policy` (ask / auto / never) decides what pinning does with a
  GitHub HTTPS remote; `status`, `doctor` and `pre-push` warn about one.
- GitHub's published host keys are added to `~/.ssh/known_hosts` (backup first),
  so the first SSH push never stops at a yes/no prompt.
- `core.sshCommand` runs Windows OpenSSH when present, so a passphrase
  remembered by the Windows ssh-agent works in the terminal and in VS Code;
  the app's "Remember my passphrase" runs `ssh-add` in a console and can turn
  the agent on (one UAC prompt).

### Safety layers
- `pre-commit`: blocks a commit whose author or committer email doesn't match
  the repo's profile.
- `pre-push`: blocks pushing commits with the wrong email; warns when the SSH
  key isn't the profile's key.
- `githop fix`: re-stamps unpushed wrong-identity commits after saving a backup
  ref; never touches pushed commits.
- Enforce modes `block` (default), `warn` and `block-strict`. Hooks fail open
  on GitHop's own errors unless `block-strict`.
- Hooks chain to each repo's own `.git/hooks` and to any previous global
  `core.hooksPath`; pass-through stubs keep every other client hook working.
- Husky / local `core.hooksPath` detection with a one-line snippet to add.
- `GITHOP_DISABLE=1` skips GitHop's check for one command.

### Command line (`githop.exe`)
- `init`, `add`, `edit`, `remove`, `list`, `use`, `status`, `pin`, `unpin`,
  `scan`, `keygen`, `test`, `doctor`, `fix`, `export`, `import`, `wipe`, `check`.
- `--json` output for `list`, `status`, `scan`, `doctor`, `test`; `NO_COLOR`
  and an ASCII fallback (`GITHOP_ASCII=1`).
- Exit codes: 0 ok · 1 blocked · 2 usage error · 3 internal error.

### SSH
- `githop keygen` creates ed25519 keys (passphrase on by default) restricted to
  your Windows account.
- `core.sshCommand` uses `-o IdentitiesOnly=yes` so ssh offers only the
  profile's key.
- `githop doctor` reports `Host github.com` blocks in `~/.ssh/config` that can
  offer the wrong key, and the ssh-agent service state.
- `githop test` checks a key against GitHub — the only network action, and only
  when you run it.

### Export and import
- `githop export <file> [--include-keys]`: profiles, default profile and a
  settings subset in an [age](https://age-encryption.org)-encrypted file
  (password, scrypt). Key paths are stored as file names only; backups, logs,
  pins and hooks state are never exported.
- `githop import <file>`: strict checks (size limit, exact schema, safe
  `githop_<id>` key names only), preview of every clash, and a choice per
  profile: skip, replace or rename. Existing key files are never overwritten
  unless you choose replace (the old key is renamed, not deleted). Imported keys
  are restricted to your Windows account.

### Tray app
- Tauri 2 tray app: tray icon in the default profile's colour, profile switcher,
  profiles, repos, fix, settings and onboarding screens (see Design.md).

### Install, uninstall and wipe
- Per-user NSIS installer that puts `githop` on your PATH; works offline.
- `githop wipe` (and the uninstaller) restores your original git settings,
  removes the hooks and deletes GitHop's data; SSH keys are deleted only when
  you ask.
- Every global git config change is backed up first.

### Privacy
- No network code, telemetry, analytics, crash reporting or auto-updater;
  `cargo deny` bans HTTP/WebSocket client crates.
- Fonts and icons are bundled. No secrets are stored — only paths to SSH keys.
