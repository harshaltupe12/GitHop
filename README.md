# GitHop

Hop between git identities on one Windows PC — safely, locally, automatically.

GitHop gives each identity (Office, Personal, …) its own name, email and SSH key,
picks the right one for every repo from its owner or folder, and **blocks commits
and pushes made with the wrong identity** before they leave your machine. A
command-line tool (`githop.exe`) does the work; a small tray app is planned on top.

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
