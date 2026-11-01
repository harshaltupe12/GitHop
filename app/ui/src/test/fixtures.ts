import type {
  AppState,
  FixPlan,
  HookHealth,
  Profile,
  ProfileSet,
  RepoRow,
  ScannedRepo,
  Settings,
} from '../lib/types';

export const office: Profile = {
  id: 'office',
  label: 'Office',
  name: 'Harshal Office',
  email: 'harshal@company.com',
  color: 'blue',
  ssh_key: 'C:/Users/harshal/.ssh/githop_office',
  owners: ['office-org'],
  folders: ['C:/Office/'],
};

export const personal: Profile = {
  id: 'personal',
  label: 'Personal',
  name: 'Harshal',
  email: '12345+harshal@users.noreply.github.com',
  color: 'violet',
  ssh_key: 'C:/Users/harshal/.ssh/githop_personal',
  owners: ['harshal-personal'],
  folders: [],
};

export const profileSet = (over: Partial<ProfileSet> = {}): ProfileSet => ({
  version: 1,
  default: 'personal',
  profile: [office, personal],
  ...over,
});

export const settings = (over: Partial<Settings> = {}): Settings => ({
  version: 1,
  logging: false,
  log_retention_days: 7,
  enforce: 'block',
  hooks_installed: true,
  previous_hooks_path: '',
  githop_exe: 'C:/Program Files/GitHop/githop.exe',
  hook_hashes: {},
  pinned_repos: [],
  github_host_aliases: [],
  hinted_repos: [],
  managed_global_keys: [],
  all_hook_stubs: false,
  remote_https_policy: 'ask',
  ...over,
});

export const hooks = (over: Partial<HookHealth> = {}): HookHealth => ({
  installed: true,
  hooks_dir: 'C:/Users/harshal/AppData/Roaming/githop/hooks',
  global_hooks_path: 'C:/Users/harshal/AppData/Roaming/githop/hooks',
  points_to_us: true,
  overridden_by: [],
  included_hooks_paths: [],
  missing: [],
  tampered: [],
  stale: [],
  githop_exe: 'C:/Program Files/GitHop/githop.exe',
  exe_missing: false,
  previous_hooks_path: '',
  previous_missing: false,
  sh_path: 'C:/Program Files/Git/bin/sh.exe',
  config_error: null,
  ...over,
});

export const appState = (over: Partial<AppState> = {}): AppState => ({
  profiles: profileSet(),
  settings: settings(),
  hooks: hooks(),
  onboarding: {
    has_profiles: true,
    hooks_installed: true,
    global_identity: { name: 'Harshal', email: 'harshal@company.com' },
    step_hint: 'done',
    backup_dir: 'C:/Users/harshal/AppData/Roaming/githop/backups',
  },
  autostart: true,
  ...over,
});

export const emptyState = (): AppState =>
  appState({
    profiles: { version: 1, default: null, profile: [] },
    settings: settings({ hooks_installed: false }),
    hooks: hooks({ installed: false, points_to_us: false, global_hooks_path: null }),
    onboarding: {
      has_profiles: false,
      hooks_installed: false,
      global_identity: { name: 'Harshal', email: 'harshal@company.com' },
      step_hint: 'welcome',
      backup_dir: 'C:/Users/harshal/AppData/Roaming/githop/backups',
    },
  });

export const repo = (
  root: string,
  over: Partial<ScannedRepo['resolution']> = {},
  extra: Partial<RepoRow> = {},
): RepoRow => ({
  root,
  resolution: {
    profile_id: 'personal',
    source: 'pin',
    remote_name: 'origin',
    remote: { host: 'github.com', owner: 'harshal-personal', repo: root.split('/').pop() ?? 'r' },
    pinned: 'personal',
    warnings: [],
    ...over,
  },
  identity: {
    author: { name: 'Harshal', email: personal.email },
    committer: { name: 'Harshal', email: personal.email },
  },
  error: null,
  unpushed_offenders: 0,
  https_remotes: [],
  ssh_switched: [],
  switched: [],
  ...extra,
});

export const offender = (sha: string, subject: string, email = office.email) => ({
  sha,
  subject,
  author_email: email,
  committer_email: email,
  author_mismatch: true,
  committer_mismatch: true,
});

export const plan = (over: Partial<FixPlan> = {}): FixPlan => ({
  repo: 'C:/Personal/blog',
  branch: 'main',
  profile_id: 'personal',
  profile_email: personal.email,
  offenders: [offender('a1b2c3d4e5f6', 'Add post'), offender('0f9e8d7c6b5a', 'Fix typo')],
  pushed_offenders: [],
  blocked_reason: null,
  ...over,
});

/** A GitHub HTTPS remote as the backend reports it (ADR 0006). */
// (Built from parts: src/ may not contain a literal web URL, tokens.test.ts.)
export const httpsRemote = (owner: string, name: string) => ({
  name: 'origin',
  url: ['https:', '', 'github.com', owner, `${name}.git`].join('/'),
  ssh_url: `git@github.com:${owner}/${name}.git`,
});
