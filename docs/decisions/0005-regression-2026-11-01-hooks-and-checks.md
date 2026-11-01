# 5. Regression 2026-11-01: strict mode at run time, pins, aliases, real history

Date: 2026-11-01 · Status: accepted (pending owner review)

## Context

The CLI/hooks regression of 2026-11-01 (D1–D23) and the security regression
of the same day found that: SSH host aliases only worked after a `scan` or
`doctor`; the commit check pinned repos matched only by the default profile
(overwriting the user's own local `user.email`); a mode changed by hand in
`settings.toml` left a stale `STRICT=1` in the hooks; pushes made with
`GITHOP_DISABLE=1` weren't recorded, so Fix could rewrite them; and replace
refs or `info/grafts` could make the push check pass a commit the remote then
receives with the wrong email.

## Decisions

1. **block-strict is read at run time, not baked in** (supersedes ADR 0004
   #7). `githop check` already exits 1 for its own error under block-strict,
   so the scripts treat exit 3 as "fail open, already explained" in every
   mode. Only when `githop.exe` is missing or exits with another code do the
   scripts read `enforce` from `settings.toml` (absolute path rendered into
   the script), with shell built-ins only (`while read`, `case`; no process
   start), stopping at the first `[table]`. The normal path costs nothing
   extra. The hooks no longer change with the mode, so nothing goes stale and
   no re-render is needed (`refresh_if_installed` is kept for upgrades). We
   chose this over a `strict` marker file because a hand edit of
   `settings.toml` can't leave the marker out of date.
2. **`hooks install` needs only `settings.toml`** (`Store::update_settings_at`),
   so it repairs hooks while `profiles.toml` is broken.
3. **The commit check pins only on an owner or folder match** (extends ADR
   0003 #6 from `scan --apply` and clone to the commit check). A default-only
   mismatch is blocked (or warned) with "pin the right profile", and nothing
   in the repo's config is written. `status` no longer suggests a
   default-only pin.
4. **SSH host alias cache.** `init`, `add`, `edit`, `use`, `hooks install`,
   `scan` and `doctor` run the full lookup (`ssh -G`) and store the size and
   mtime of `~/.ssh/config` (`settings.ssh_config_stamp`). Hooks and `status`
   compare that stamp (one `stat`); when it differs they parse the file in
   process (`Host` → literal `HostName github.com`; aliases confirmed by
   `ssh -G` before are kept while their `Host` block exists) and save the
   cache best effort. Never `ssh -G` in hooks; the commit fast path is still
   one `git` spawn.
5. **`GITHOP_DISABLE=1` push still records.** `pre-push` hands the ref lines to
   `githop check`, which records them in `.git/githop/pushed` (no spawn) and
   returns `disabled` without checking. Commit-time hooks still skip
   `githop.exe` entirely.
6. **Checks and Fix read real history**: `GIT_NO_REPLACE_OBJECTS=1` and
   `GIT_GRAFT_FILE` pointing at a file that doesn't exist, on every check
   stage's `Git` handle and Fix's (env only, no extra spawn).
7. **Pin vs owner.** The resolver reports `PinOwnerDisagree` when a pin decides
   but the owner (or folder) rule names another profile. The pin still wins;
   `status`, `doctor` (in a repo), `scan` and `edit` show it with the
   `githop pin` command.
8. **Fix undo checks the pair and publication**: tree(backup) == tree(result),
   equal commit counts on both sides, and no commit in `backup..result`
   reachable from a remote-tracking ref, a tag or a recorded push.
9. **Terminal-safe output.** Every human line the CLI prints goes through one
   filter that turns control characters (ESC, CSI, OSC, BEL, C1, bidi
   overrides) into `?`; repo-sourced values in core messages (subjects,
   emails, tag names) are made one-line. JSON is escaped by serde.
10. **Profile shape rules live in core** (`ProfileSet::issues`): no control
   characters in label, name, email, key path, owners or folders; no `"` or
   `..` in the key path; caps (label/name 128, email 254, paths 4096, lists
   100). `pin`, `use` and Fix refuse a profile with errors.

## Consequences

- `githop hooks snippet push` is three lines (it buffers stdin and hands it
  back with a here-document).
- In submodules, `post-checkout` runs the clone check on every checkout (git
  checks a new submodule out with a non-null previous HEAD); only a pin is
  acted on there, quietly.
- Wipe removes `refs/githop/*` and `.git/githop/pushed` from the repos it
  unpins, and in its data folder deletes only the files it writes.
