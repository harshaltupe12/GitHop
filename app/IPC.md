# GitHop tray app — IPC contract

Binding contract between `app/src-tauri` (Rust commands) and `app/ui` (`src/lib/ipc.ts`).
DTO field names are **snake_case** exactly as the core types serialise with serde.
Tauri command argument names are snake_case in Rust; the UI passes camelCase keys
(Tauri 2 default mapping), e.g. `invoke("save_profile", { profile, originalId })`.

Every command returns `Result<T, IpcError>`. The UI never receives a key passphrase.

## Shared types

Core types re-used as-is (see `crates/githop-core/src`): `Profile`, `ProfileColor`
(`"blue" | "violet" | "pink" | "indigo" | "plum" | "slate" | "navy" | "steel"`),
`ProfileSet { version, default?, profile: Profile[] }` (note: TOML/serde key `profile`),
`Settings`, `EnforceMode` (`"block" | "warn" | "block-strict"`), `ValidationIssue
{ profile_id?, field, severity: "error"|"warning"|"info", message }`, `Identity { name, email }`,
`EffectiveIdentity { author, committer }`, `RemoteInfo { host, owner, repo }`,
`Resolution { profile_id?, source?: "pin"|"owner"|"folder"|"default", remote_name?, remote?, pinned?, warnings: ResolutionWarning[] }`,
`ResolutionWarning` (tagged `kind`: `owner-folder-disagree | stale-pin | unknown-remote-format | non-github-host | local-hooks-path`),
`ScannedRepo { root, resolution, identity?, error? }`,
`ConnectionTest` (tagged `kind`: `connected {user} | denied | failed {reason}`),
`HookHealth` (core `hooks::HookHealth`), `OffendingCommit { sha, subject, author_email, committer_email, author_mismatch, committer_mismatch }`.

```ts
type IpcError = {
  code: "validation" | "locked" | "conflict" | "not_found" | "not_a_repo"
      | "git" | "io" | "unsupported" | "bad_input" | "internal";
  message: string;            // user-facing, Design §9 voice
  issues: ValidationIssue[];  // non-empty only for "validation"
};
```

New core types (owned by the agent named; must serialise exactly like this):

```ts
// core `doctor` (CLI agent)
type DoctorCheck = { id: string; status: "ok" | "warning" | "problem"; title: string; detail: string; fix?: string };
type DoctorReport = { checks: DoctorCheck[] };

// core `fix` (Phase 4 agent)
type FixPlan = {
  repo: string; branch?: string; profile_id?: string; profile_email?: string;
  offenders: OffendingCommit[];        // unpushed, wrong identity — will be re-stamped
  pushed_offenders: OffendingCommit[]; // already on a remote — never touched
  blocked_reason?: string;             // e.g. dirty tree, rebase in progress, detached HEAD, unresolved
  blocker?: "operation-in-progress" | "detached-head" | "unresolved" | "dirty-tree" | "too-many-commits";
  unpushed: number;                    // commits on no remote-tracking ref
  rewrite_count: number;               // offenders + their unpushed descendants (get new ids)
  signed: number;                      // signed commits whose signature will be dropped
  has_remote_refs: boolean;
};
type FixResult = {
  backup_ref: string; rewritten: number; undo_hint: string;
  recreated: number; old_head: string; new_head: string;
};

// core `wipe` (CLI agent)
type WipeOptions = { unpin_repos: boolean; delete_keys: boolean; keep_data: boolean };
type WipeStep = { step: string; ok: boolean; detail: string };
type WipeReport = { steps: WipeStep[] };

// core `export` (Phase 6 agent)
type ImportConflict = { profile_id: string; kind: "id" | "email" | "color" | "owner" | "key_file"; existing_id: string };
type ImportPreview = { profiles: Profile[]; conflicts: ImportConflict[]; keys: string[]; include_settings: boolean };
type ImportResolution = { profile_id: string; action: "skip" | "replace" | "rename"; new_id?: string };
```

App-only DTOs (defined in `app/src-tauri/src/dto.rs`):

```ts
type OnboardingState = { has_profiles: boolean; hooks_installed: boolean; global_identity?: Identity; step_hint: string };
type AppState = { profiles: ProfileSet; settings: Settings; hooks: HookHealth; onboarding: OnboardingState; autostart: boolean };
type SettingsPatch = { enforce?: EnforceMode; logging?: boolean; autostart?: boolean };
type AppInfo = { version: string; licenses: string };
type ScanProgress = { folder: string; found: number; done: number };
```

## Commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `get_state` | — | `AppState` | Called on start + window focus |
| `list_profiles` | — | `ProfileSet` | |
| `validate_profile` | `profile: Profile, original_id?: string` | `ValidationIssue[]` | No write; for live form validation |
| `save_profile` | `profile: Profile, original_id?: string` | `ProfileSet` | Add (no original_id) or edit (rename allowed) |
| `delete_profile` | `id: string, new_default?: string` | `ProfileSet` | Default needs `new_default` (or it's the last profile) |
| `set_default` | `id: string` | `ProfileSet` | Backs up global config first time; updates tray |
| `suggest_key_path` | `id: string` | `string` | `~/.ssh/githop_<id>` |
| `generate_key` | `id: string, no_passphrase: boolean` | `string` (public key) | Opens a visible console for the passphrase unless `no_passphrase` |
| `read_public_key` | `id: string` | `string` | |
| `test_connection` | `id: string` | `ConnectionTest` | **Network, user click only** |
| `scan` | `folder: string` | `ScannedRepo[]` | Worker thread; emits `scan://progress` (`ScanProgress`) |
| `pin` | `repo: string, id: string` | `ScannedRepo` | |
| `unpin` | `repo: string` | `ScannedRepo` | |
| `pin_all` | `repos: string[]` | `ScannedRepo[]` | Pins each to its resolved profile |
| `hooks_snippet` | `stage: "commit" \| "push" \| "clone"` | `string` | For husky "How to fix" dialog |
| `fix_plan` | `repo: string` | `FixPlan` | |
| `fix_apply` | `repo: string` | `FixResult` | |
| `fix_undo` | `repo: string, backup_ref: string` | `void` | |
| `doctor` | — | `DoctorReport` | |
| `update_settings` | `patch: SettingsPatch` | `Settings` | |
| `install_hooks` | — | `HookHealth` | Onboarding; backs up global config first |
| `wipe` | `opts: WipeOptions` | `WipeReport` | App quits after success |
| `export_profiles` | `path: string, password: string, include_keys: boolean` | `void` | Password ≥ 12 chars |
| `import_preview` | `path: string, password: string` | `ImportPreview` | |
| `import_apply` | `path: string, password: string, resolutions: ImportResolution[]` | `ProfileSet` | |
| `open_external` | `target: "ssh-keys" \| "releases"` | `void` | Opens the user's browser (opener plugin, scoped URLs) |
| `app_info` | — | `AppInfo` | About + licences |

## Events (Rust → UI)

| Event | Payload | When |
|---|---|---|
| `profiles://changed` | — | `profiles.toml`/`settings.toml` changed on disk (CLI edit) or by a command |
| `scan://progress` | `ScanProgress` | During `scan` |
| `tray://default-changed` | `ProfileSet` | Default switched from the tray menu |
