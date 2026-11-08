// DTOs mirrored from app/IPC.md and crates/githop-core (serde, snake_case).
// Rust `Option<T>` serialises as `null` (or is omitted with skip_serializing_if),
// so optional fields are typed `?: T | null`.

export const PROFILE_COLORS = [
  'blue',
  'violet',
  'pink',
  'indigo',
  'plum',
  'slate',
  'navy',
  'steel',
] as const;
export type ProfileColor = (typeof PROFILE_COLORS)[number];

export type Profile = {
  id: string;
  label: string;
  name: string;
  email: string;
  color: ProfileColor;
  ssh_key: string;
  owners: string[];
  folders: string[];
  /** Sign commits with the SSH key (GitHub "Verified"). Absent = off. */
  sign_commits?: boolean;
};

export type ProfileSet = {
  version: number;
  default?: string | null;
  profile: Profile[];
};

export type EnforceMode = 'block' | 'warn' | 'block-strict';

/** What pinning does with a GitHub HTTPS remote (ADR 0006). */
export type RemoteHttpsPolicy = 'ask' | 'auto' | 'never';

export type Settings = {
  version: number;
  logging: boolean;
  log_retention_days: number;
  enforce: EnforceMode;
  hooks_installed: boolean;
  previous_hooks_path: string;
  githop_exe: string;
  hook_hashes: Record<string, string>;
  pinned_repos: string[];
  github_host_aliases: string[];
  hinted_repos: string[];
  managed_global_keys: string[];
  /** Also install the optional hook stubs (reference-transaction, post-index-change). */
  all_hook_stubs: boolean;
  /** GitHub HTTPS remotes on pin: ask (default), switch to SSH, or leave them. */
  remote_https_policy: RemoteHttpsPolicy;
};

export type Severity = 'error' | 'warning' | 'info';

export type ValidationIssue = {
  profile_id?: string | null;
  field: string;
  severity: Severity;
  message: string;
};

export type Identity = { name: string; email: string };
export type EffectiveIdentity = { author: Identity; committer: Identity };
export type RemoteInfo = { host: string; owner: string; repo: string };

export type ResolutionSource = 'pin' | 'owner' | 'folder' | 'default';

export type ResolutionWarning =
  | { kind: 'owner-folder-disagree'; owner_profile: string; folder_profile: string }
  | { kind: 'stale-pin'; profile_id: string }
  | { kind: 'unknown-remote-format'; remote: string }
  | { kind: 'non-github-host'; host: string }
  | { kind: 'local-hooks-path'; path: string }
  | {
      kind: 'pin-owner-disagree';
      pinned_profile: string;
      rule_profile: string;
      rule: 'owner' | 'folder';
      owner?: string | null;
    };

export type Resolution = {
  profile_id?: string | null;
  source?: ResolutionSource | null;
  remote_name?: string | null;
  remote?: RemoteInfo | null;
  pinned?: string | null;
  warnings: ResolutionWarning[];
};

export type ScannedRepo = {
  root: string;
  resolution: Resolution;
  identity?: EffectiveIdentity | null;
  error?: string | null;
};

/**
 * What `scan` / `pin` / `pin_all` return (IPC.md): `ScannedRepo` plus the
 * app-side count of unpushed wrong-identity commits (0 when unresolved or blocked).
 */
export type RepoRow = ScannedRepo & {
  unpushed_offenders: number;
  /** GitHub HTTPS remotes still in place: pushes sign in with Windows' saved account. */
  https_remotes: HttpsRemote[];
  /** Remotes GitHop switched to SSH (undo available). */
  ssh_switched: string[];
  /** Remotes switched to SSH by the call that returned this row. */
  switched: string[];
};

/** A GitHub HTTPS remote (`url` never contains a password or token). */
export type HttpsRemote = { name: string; url: string; ssh_url: string };

/** Key panel: is the passphrase remembered by the Windows ssh-agent? */
export type PassphraseState =
  'no-key' | 'no-passphrase' | 'remembered' | 'asks-each-time' | 'agent-off' | 'unknown';
export type RememberOutcome =
  'remembered' | 'no-passphrase' | 'agent-off' | 'cancelled' | 'not-added';

/** Result of Generate key (works before the profile is saved). */
export type GeneratedKey = {
  public_key: string;
  /** Private key path; saved with the profile. */
  path: string;
  /** An existing GitHop key at `path` was used instead of a new one. */
  reused: boolean;
  /** Whether the Windows ssh-agent now holds the key (no prompt on push). */
  remembered: RememberOutcome;
};

export type ConnectionTest =
  { kind: 'connected'; user: string } | { kind: 'denied' } | { kind: 'failed'; reason: string };

/** A `core.hooksPath` set by a file the global git config includes (Plan C2). */
export type IncludedHooksPath = { condition?: string | null; file: string; value: string };

export type HookHealth = {
  installed: boolean;
  hooks_dir: string;
  global_hooks_path?: string | null;
  points_to_us: boolean;
  /** Includes read after GitHop's setting that override it: GitHop's hooks don't run there. */
  overridden_by: IncludedHooksPath[];
  /** Every `core.hooksPath` set by an included file. */
  included_hooks_paths: IncludedHooksPath[];
  missing: string[];
  tampered: string[];
  stale: string[];
  /** Files in GitHop's hooks folder that GitHop didn't write (they'd run for every repo). */
  unexpected?: string[];
  githop_exe: string;
  exe_missing: boolean;
  previous_hooks_path: string;
  previous_missing: boolean;
  sh_path?: string | null;
  config_error?: string | null;
};

export type OffendingCommit = {
  sha: string;
  subject: string;
  author_email: string;
  committer_email: string;
  author_mismatch: boolean;
  committer_mismatch: boolean;
};

export type IpcErrorCode =
  | 'validation'
  | 'locked'
  | 'conflict'
  | 'not_found'
  | 'not_a_repo'
  | 'git'
  | 'io'
  | 'unsupported'
  | 'bad_input'
  | 'internal';

export type IpcError = {
  code: IpcErrorCode;
  message: string;
  issues: ValidationIssue[];
};

export type DoctorCheck = {
  id: string;
  status: 'ok' | 'warning' | 'problem';
  title: string;
  detail: string;
  fix?: string | null;
};
export type DoctorReport = { checks: DoctorCheck[] };

export type FixPlan = {
  repo: string;
  branch?: string | null;
  profile_id?: string | null;
  profile_email?: string | null;
  offenders: OffendingCommit[];
  pushed_offenders: OffendingCommit[];
  blocked_reason?: string | null;
  blocker?:
    | 'operation-in-progress'
    | 'detached-head'
    | 'unresolved'
    | 'dirty-tree'
    | 'too-many-commits'
    | 'pushed-unknown'
    | 'shallow'
    | 'invalid-profile'
    | null;
  unpushed?: number;
  rewrite_count?: number;
  signed?: number;
  has_remote_refs?: boolean;
  /** Unpushed commits by someone else: copied exactly, author kept. */
  foreign?: number;
};
export type FixResult = {
  backup_ref: string;
  rewritten: number;
  undo_hint: string;
  recreated?: number;
  old_head?: string;
  new_head?: string;
  /** The new head fast-forwards from upstream / last push: a plain push works. */
  fast_forward?: boolean;
};

export type WipeOptions = { unpin_repos: boolean; delete_keys: boolean; keep_data: boolean };
export type WipeStep = { step: string; ok: boolean; detail: string };
export type WipeReport = { steps: WipeStep[] };

export type ImportConflictKind = 'id' | 'email' | 'color' | 'owner' | 'key_file';
export type ImportConflict = { profile_id: string; kind: ImportConflictKind; existing_id: string };
export type ImportPreview = {
  profiles: Profile[];
  conflicts: ImportConflict[];
  keys: string[];
  include_settings: boolean;
};
export type ImportAction = 'skip' | 'replace' | 'rename';
export type ImportResolution = { profile_id: string; action: ImportAction; new_id?: string | null };

export type OnboardingState = {
  has_profiles: boolean;
  hooks_installed: boolean;
  global_identity?: Identity | null;
  step_hint: string;
  /** Where GitHop keeps its backups (forward slashes). */
  backup_dir: string;
};
export type AppState = {
  profiles: ProfileSet;
  settings: Settings;
  hooks: HookHealth;
  onboarding: OnboardingState;
  autostart: boolean;
};
export type SettingsPatch = {
  enforce?: EnforceMode;
  logging?: boolean;
  autostart?: boolean;
  remote_https_policy?: RemoteHttpsPolicy;
};
export type AppInfo = { version: string; licenses: string };
export type ScanProgress = { folder: string; found: number; done: number };

export type HookStage = 'commit' | 'push' | 'clone';
export type ExternalTarget = 'ssh-keys' | 'releases' | 'docs';
