# Contributing to GitHop

Thanks for helping make GitHop better. Bug reports, ideas, docs fixes and code are all welcome.

## Ways to help

- **Found a bug?** Open an [issue](../../issues/new/choose) with the steps to reproduce it and the output of `githop doctor`.
- **Have an idea?** Start a [discussion](../../discussions) or open a feature request.
- **Want to code?** Look for issues labelled `good first issue` or `help wanted`, and leave a comment so others know you're on it.

## Before you start

GitHop has a few rules that every change has to keep:

1. **No network code.** GitHop never makes a network request. No HTTP clients, telemetry, analytics or auto-updaters. `cargo deny` checks this.
2. **Never store secrets.** Only paths to SSH keys, never passwords, tokens or passphrases.
3. **Hooks fail open.** If GitHop itself errors inside a git hook, it warns and lets git continue. Only a real identity mismatch blocks.
4. **Never rewrite pushed commits.** Fix only touches commits that aren't on any remote, and always saves a backup first.
5. **Back up before writing** the user's git config, and write files atomically.
6. **Tests never touch your real git or SSH config.** Every test runs in an isolated temp environment.

## Setting up

You need Windows 10 or 11, [Rust](https://rustup.rs) (MSVC toolchain), [Git for Windows](https://git-scm.com/download/win) 2.40+ and Node.js LTS.

```powershell
git clone https://github.com/harshaltupe12/GitHop.git
cd GitHop
cargo build
npm --prefix app/ui ci
npm --prefix app ci
```

Run the app with `cd app; npm run tauri dev`.

## Checks to run before a pull request

```powershell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check
cd app/ui; npm run lint; npm run typecheck; npm test
```

All of them should pass.

## Pull requests

- Keep each pull request focused on one change.
- Add or update tests for what you changed.
- For anything user-facing, use plain, calm wording and sentence case, like the rest of the app.
- Describe what changed and why, and add a screenshot for UI changes.

## Project layout

```text
crates/githop-core/   core library: profiles, resolver, git config, hooks, checks, fix
crates/githop-cli/    githop.exe: the command-line tool and git hook entry point
app/src-tauri/        GitHopApp.exe: Tauri 2 tray app and installer
app/ui/               React + TypeScript + Tailwind screens
brand/                logo, icons and usage rules
```

By contributing, you agree that your contributions are licensed under the project's MIT or Apache-2.0 licence, and you agree to follow our [Code of Conduct](CODE_OF_CONDUCT.md).
