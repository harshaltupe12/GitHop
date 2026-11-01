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
   **A commit email is "wrong" only when it is one of the user's own identities** and
   not the owning profile's: any other profile's email, the global `user.email`
   (`author.email`/`committer.email`), the email in `gitconfig.original`, `$EMAIL`, or
   git's auto-detected `<user>@<host>` / `(none)` ident — case-insensitive, the same
   rule for the push check and Fix (`check::identity_rules`). Someone else's commits (a
   merged PR, `git am`, a cherry-pick) never block a push and Fix never re-stamps
   them; when only the committer is wrong, only the committer changes and the foreign
   author is kept exactly. Annotated tags' tagger emails follow the same rule on push.
   **"Pushed" is conservative**: reachable from any remote-tracking ref, any tag
   (local ones too), a sha the pre-push check recorded in `.git/githop/pushed`, or an
   upstream that has no remote-tracking ref (Fix refuses on that branch; other such
   branches count as pushed up to their tip). Shallow clones are refused unless the
   branch has a remote-tracking upstream.
   `pushed_offenders` = recent pushed commits on HEAD using another of the user's emails.
5. **Fix backups: keep the newest 10 per branch** (refs under `refs/githop/backup/`).
6. **`scan --apply` pins only owner/folder matches**; default-only matches need
   `--include-default`, so an office repo can't be pinned to a personal default by accident.
7. **CLI exit code 1 also means "a check found a problem"** (`doctor` problems,
   `test` denied) in addition to "blocked".
8. **Planning docs (CLAUDE.md, Plan.md, Design.md, Implementation/) are gitignored**
   at the owner's request; they remain the local source of truth.
9. **Licence: dual `MIT OR Apache-2.0`.**
10. **`reference-transaction` and `post-index-change` pass-through stubs are not
    installed by default.** git fires them ~6× per commit; each stub is an `sh`
    start (~30–50 ms on Windows), which dominated commit latency. They are installed
    when the previous global hooks folder has them or with
    `githop hooks install --all-stubs`; `githop doctor` warns when a pinned repo has
    its own copy (C2 trade-off, explicit).
11. **Identity env vars equal to the configured value are not overrides.** `git commit`
    exports `GIT_AUTHOR_NAME/EMAIL/DATE` to its hooks; only a value that differs from
    config counts as a C9 override. Keeps the 1-spawn fast path under real hooks.
12. **export/import uses `age` 0.11** (0.12 pulls a crate with an open advisory).

## Consequences

Plan.md / Design.md decision logs should be updated to match after review.
