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
};

export type ProfileSet = {
  version: number;
  default?: string | null;
  profile: Profile[];
};

export type EnforceMode = 'block' | 'warn' | 'block-strict';

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
  | { kind: 'local-hooks-path'; path: string };

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

export type ConnectionTest =
  { kind: 'connected'; user: string } | { kind: 'denied' } | { kind: 'failed'; reason: string };

export type HookHealth = {
  installed: boolean;
  hooks_dir: string;
  global_hooks_path?: string | null;
  points_to_us: boolean;
  missing: string[];
  tampered: string[];
  stale: string[];
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
};
export type FixResult = { backup_ref: string; rewritten: number; undo_hint: string };

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
};
export type AppState = {
  profiles: ProfileSet;
  settings: Settings;
  hooks: HookHealth;
  onboarding: OnboardingState;
  autostart: boolean;
};
export type SettingsPatch = { enforce?: EnforceMode; logging?: boolean; autostart?: boolean };
export type AppInfo = { version: string; licenses: string };
export type ScanProgress = { folder: string; found: number; done: number };

export type HookStage = 'commit' | 'push' | 'clone';
export type ExternalTarget = 'ssh-keys' | 'releases';
