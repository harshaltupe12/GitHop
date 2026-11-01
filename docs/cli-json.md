# `githop --json` output

`--json` prints exactly one pretty-printed JSON document on **stdout** and
nothing else there. Human messages, warnings and errors still go to
**stderr**. The exit code is the same as without `--json`
(`0` ok · `1` blocked / problem found · `2` usage error · `3` internal error).

Field names are `snake_case`. Paths use forward slashes. Optional values are
`null` (or left out where noted). Enum values are lower-case / kebab-case
strings. The tray app calls `githop-core` directly; these schemas are for
scripts and tests. Types written as `Resolution`, `Identity`, … are the core
types listed in [`app/IPC.md`](../app/IPC.md), serialised the same way.

Commands without a JSON form (`init`, `add`, `edit`, `remove`, `use`, `pin`,
`unpin`, `keygen`, `hooks install|uninstall|snippet`) refuse `--json`
with exit `2`.

---

## githop list

```ts
{
  default: string | null;          // default profile id
  profiles: {
    id: string; label: string; name: string; email: string;
    color: "blue" | "violet" | "pink" | "indigo" | "plum" | "slate" | "navy" | "steel";
    ssh_key: string;               // private key path
    owners: string[];              // GitHub users/orgs (lower-case)
    folders: string[];             // normalised, trailing "/"
    default: boolean;
  }[];
}
```

## githop status

Run inside a repo (outside one → exit `2`).

```ts
{
  repo: string;                    // repo root
  profile: { id; label; name; email; ssh_key } | null;   // owning profile
  source: "pin" | "owner" | "folder" | "default" | null;
  pinned: string | null;           // value of githop.profile, even if stale
  resolution: Resolution;
  identity: EffectiveIdentity | null;   // what the next commit would use
  identity_matches: boolean | null;     // null when unresolved
  ssh_command: string | null;      // effective core.sshCommand
  ssh_key: string | null;          // key path parsed from it
  key_matches: boolean | null;
  warnings: (ResolutionWarning & { message: string })[];
}
```

## githop scan &lt;folder&gt;

```ts
{
  folder: string;
  repos: {
    root: string;
    status: "pinned" | "pinned-now" | "not-pinned" | "wrong-identity"
          | "stale-pin" | "unresolved" | "error";
    profile_id: string | null;
    source: "pin" | "owner" | "folder" | "default" | null;
    resolution: Resolution;
    identity: EffectiveIdentity | null;
    error: string | null;
    warnings: (ResolutionWarning & { message: string })[];
  }[];
  applied: string[];               // roots pinned by --apply
  apply_errors: { root: string; error: string }[];
}
```

`scan` also refreshes `settings.github_host_aliases` (SSH host aliases whose
`HostName` is `github.com`, confirmed with `ssh -G`).

## githop test &lt;id&gt;

Connects to github.com (user-initiated, the only network use).

```ts
{
  profile: string;                 // profile id
  key: string;                     // key path used
  result: { kind: "connected"; user: string }
        | { kind: "denied" }
        | { kind: "failed"; reason: string };
}
```

## githop doctor

Exit `1` when any check is a `problem`.

```ts
{
  checks: DoctorCheck[];
  summary: { ok: number; warning: number; problem: number };
}

// core `githop_core::doctor::DoctorCheck` (same shape as app/IPC.md)
type DoctorCheck = {
  id: string;                      // see below; several checks may share an id
  status: "ok" | "warning" | "problem";   // "ok" with a fix = a note
  title: string;                   // what was checked: "Git", "Hooks", "SSH", …
  detail: string;                  // one sentence
  fix?: string;                    // one next step; left out when none
};
```

Check ids: `git-found`, `git-version`, `ssh-found`, `ssh-keygen-found`,
`store`, `profiles`, `profile-issue`, `default`, `key-file`, `key-pub`,
`key-acl` (Unix only), `original`, `hooks-installed`, `hooks-path`,
`hooks-files`, `hooks-stale`, `hooks-exe`, `hooks-previous`, `hooks-sh`,
`local-hooks-path` (current repo sets its own `core.hooksPath`),
`ssh-config`, `ssh-aliases`, `env`, `system-config`, `agent`.

## githop check --stage &lt;clone|commit|push&gt;

Hooks don't use `--json`; it's for scripts and tests. Messages are still
printed on stderr. Exit code: `0` pass · `1` blocked · `3` GitHop error
(fail-open) — under `enforce = "block-strict"` a GitHop error is `1`.

```ts
// core `githop_core::check::CheckResult`
{
  stage: "clone" | "commit" | "push";
  outcome:
      { kind: "pass" }
    | { kind: "pinned"; profile_id: string }        // clone auto-pinned
    | { kind: "unresolved" }
    | { kind: "mismatch"; pinned_now: string | null }
    | { kind: "internal-error"; detail: string }
    | { kind: "disabled" };                          // GITHOP_DISABLE=1
  enforce: "block" | "warn" | "block-strict";
  messages: { level: "ok" | "blocked" | "warning" | "info"; text: string; hint: string | null }[];
  profile_id: string | null;       // profile that owns the repo
  offending: OffendingCommit[];    // push: every wrong-identity commit found
  capped: boolean;                 // push: scan stopped at 5,000 commits
}

type OffendingCommit = {
  sha: string; subject: string; author_email: string; committer_email: string;
  author_mismatch: boolean; committer_mismatch: boolean;
};
```

## githop fix

Never prompts with `--json` (pass `--yes` to apply). Exit `0` fixed (or
`--dry-run` with nothing pending) · `1` refused or commits still to fix.

```ts
// githop fix --dry-run
FixPlan                            // core `githop_core::fix::FixPlan` (app/IPC.md), bare

// githop fix --yes
{
  plan: FixPlan;
  result: FixResult | null;        // { backup_ref, rewritten, undo_hint }; null when refused
}

// githop fix --undo [<backup ref>] --yes
{ undone: string | null }          // the backup ref restored; null when refused
```

## githop hooks status

Exit `1` when any hook check is a `problem`.

```ts
{
  ok: boolean;                     // HookHealth::is_ok
  health: {                        // core `githop_core::hooks::HookHealth`
    installed: boolean;
    hooks_dir: string;
    global_hooks_path: string | null;
    points_to_us: boolean;
    missing: string[];             // hook files that should exist
    tampered: string[];            // edited by hand
    stale: string[];               // from an older GitHop
    githop_exe: string;            // exe embedded in the hooks
    exe_missing: boolean;
    previous_hooks_path: string;   // chained to; "" = none
    previous_missing: boolean;
    sh_path: string | null;        // Git for Windows sh (native path, may use "\")
    config_error: string | null;
  };
  checks: DoctorCheck[];           // the hook checks doctor would show
}
```

## githop wipe

`--json` needs `--yes` (or `--from-uninstaller`): it never prompts. Exit `0`
when every step worked, `3` when one didn't (running wipe again finishes it).

```ts
// core `githop_core::wipe::WipeReport` (same shape as app/IPC.md)
{
  steps: {
    step: "backup" | "hooks" | "identity" | "unpin" | "autostart" | "data" | "keys";
    ok: boolean;
    detail: string;                // one sentence, Design §9 voice
  }[];                             // always all seven, in this order
}
```
