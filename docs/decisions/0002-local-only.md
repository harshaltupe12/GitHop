# 2. Local-only: no network crate in the dependency tree

Date: 2026-10-31 · Status: accepted

## Context

GitHop sees which identities a user has, which repos they work on and where their SSH
keys live. On an office machine that information is sensitive. A tool that could phone
home — even "just" for updates or crash reports — becomes a possible exfiltration path,
and users can't verify that it doesn't.

## Decision

- GitHop makes **no network requests**. No telemetry, analytics, crash reporting,
  auto-updater, accounts, CDN assets or web fonts.
- No HTTP/socket client crate may appear in the dependency tree. `deny.toml` bans them
  and CI runs `cargo deny check`.
- The only network traffic is git/ssh performing the user's own push/pull/clone, and
  `ssh -T git@github.com` when the user explicitly asks to test a key.
- "Check for updates" opens the releases page in the user's browser.

## Consequences

- Privacy is verifiable from the dependency tree, not just promised.
- No automatic updates; users update manually.
- When Tauri is added, any banned crate pulled in transitively must be removed by
  turning off the feature that pulls it in — never by loosening the ban list.
