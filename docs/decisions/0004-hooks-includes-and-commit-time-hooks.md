# 4. Hooks: included `core.hooksPath`, merge/sequencer commits, strict mode

Date: 2026-11-01 · Status: accepted (pending owner review)

## Context

The 2026-11-01 review (findings #1, #5, #8, #9) showed that the global hooks
could be switched off by a `core.hooksPath` coming from an `[include]` /
`[includeIf]` file, that merge, revert and cherry-pick commits skipped the
commit check, that a repo hook could run twice, and that `block-strict` did not
block when `githop.exe` was missing or crashed.

## Decisions

1. **Included `core.hooksPath` → GitHop installs through its own include, read
   last.** git uses the last value it reads. When a file the global config
   includes sets `core.hooksPath` (company hooks, often behind `includeIf`),
   `githop hooks install` leaves the user's own global value where it is,
   writes GitHop's value to `<data dir>/hooks.gitconfig`, and appends
   `[include] path = "<that file>"` at the very end of the global config
   (atomic write, after the usual backup). Uninstall removes just that line
   (and the empty `[include]` header git leaves behind) and the file, so the
   global file is byte-for-byte as before. The global file keeps one own
   `core.hooksPath` value, so `git config --global core.hooksPath X` keeps
   working for the user. Without such an include, install works as before
   (`--replace-all` in the global file).
2. **In that mode the hooks pick the folder to chain to at run time:**
   `git config --get-all core.hooksPath`, last value that isn't GitHop's. That
   is exactly the folder git would use without GitHop in that repo, so an
   `includeIf` that applies to `C:/work/` runs the company hooks only there.
   Cost: one `git config` spawn per hook run, only for users with such an
   include.
3. **Doctor reads the effective value.** Globally: an included value read
   after GitHop's is a problem ("Run: githop hooks install"). In the cwd repo:
   the effective `core.hooksPath` (any scope, `includeIf` evaluated by git for
   that repo) that isn't GitHop's is a problem; `githop status` warns too.
   `includeIf` conditions are not evaluated outside a repo: a value behind any
   condition counts as possibly applying.
4. **`pre-merge-commit` runs the commit check** (githooks(5) suggests it), so
   `git merge` / `git pull` merge commits are checked.
5. **`prepare-commit-msg` runs the commit check only for `git revert` and
   `git cherry-pick`.** The sequencer runs no `pre-commit` and, unlike
   `git commit`, does not export `GIT_AUTHOR_EMAIL` to hooks; the hook checks
   when the message source is `message` and that variable is unset, and not
   during a rebase. A failing `prepare-commit-msg` stops the revert/cherry-pick.
   `git commit` (including `--no-verify`, C15) is still checked only in
   `pre-commit`, so commit cost is unchanged. Cherry-picking a colleague's
   commit is not blocked: the check uses the configured identity (the
   committer), not the picked commit's author.
6. **The repo hook runs once** when the previous hooks path is the repo's own
   hooks folder: the scripts compare the two hook files with `-ef`.
7. **`enforce` is baked into the scripts** (`STRICT=1` for `block-strict`).
   With it, a missing `githop.exe` or any exit code other than 0 blocks.
   `hooks::refresh_if_installed` re-renders the hook files (no git config
   write); the tray's settings update calls it when `enforce` changes. Doctor
   reports hooks rendered for another mode as out of date.

## Known gaps (the push check still catches these)

- `git rebase` (and `pull --rebase`) replays commits without a commit check.
- `git cherry-pick -e/--edit` hands the commit to `git commit -n` with the
  picked commit's author exported: neither hook can check it safely.
- `git am` is not checked at commit time.
- A `GIT_AUTHOR_EMAIL` set in the user's environment hides revert/cherry-pick
  from the `prepare-commit-msg` check (doctor warns about that variable).
- Nested includes inside an included file that are themselves conditional are
  not evaluated when reading the global layout.
