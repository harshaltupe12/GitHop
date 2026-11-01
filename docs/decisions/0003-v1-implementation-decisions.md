# 3. Decisions taken during v1 implementation

Date: 2026-11-01 · Status: accepted (pending owner review)

## Context

The owner delegated implementation of all phases and asked for decisions to be taken
without waiting, with a review afterwards. These are the choices that refine or depart
from Plan.md / Design.md. Each can be revisited.

## Decisions

1. **Core API takes an explicit `Git` handle** (`Repo::discover(&git, dir)`,
   `resolve(.., github_aliases)`), so tests inject a fully isolated git environment and
   the tray app can reuse it. Refines Phase 1 §5.
2. **Push scan excludes commits reachable from *any* remote-tracking ref**, not only the
   push target's. Those commits are already published (same rule as Fix), and in fork
   workflows `upstream` commits pushed to `origin` are not the user's. Trade-off: a
   commit published on one remote and then pushed to another is not re-checked.
3. **Fix rewrites with git plumbing (`commit-tree` + compare-and-swap `update-ref`)
   instead of `rebase --exec`**: no shell string (no injection), exact trees (merges and
   conflict resolutions preserved), pushed commits never re-created, no half-rebased
   state. Known limit: commit signatures on re-created commits are dropped (warned).
4. **Fix with already-pushed offenders: fix the unpushed ones, report the pushed ones.**
   Plan §7.5 / Design §8.4 said "refuse"; that would make Fix unusable forever in any
   repo where one bad commit was ever pushed. Pushed commits are still never touched.
   `pushed_offenders` = recent pushed commits on HEAD using *another profile's* email.
5. **Fix backups: keep the newest 10 per branch** (refs under `refs/githop/backup/`).
6. **`scan --apply` pins only owner/folder matches**; default-only matches need
   `--include-default`, so an office repo can't be pinned to a personal default by accident.
7. **CLI exit code 1 also means "a check found a problem"** (`doctor` problems,
   `test` denied) in addition to "blocked".
8. **Planning docs (CLAUDE.md, Plan.md, Design.md, Implementation/) are gitignored**
   at the owner's request; they remain the local source of truth.
9. **Licence: dual `MIT OR Apache-2.0`.**

## Consequences

Plan.md / Design.md decision logs should be updated to match after review.
