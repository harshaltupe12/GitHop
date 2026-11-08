// Typed wrappers for every backend call (app/IPC.md). The only module that
// imports @tauri-apps/* — components and screens call these functions.
// Nothing here makes a network request: `invoke` goes over Tauri's local IPC.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import type {
  AppInfo,
  AppState,
  ConnectionTest,
  DoctorReport,
  ExternalTarget,
  FixPlan,
  FixResult,
  GeneratedKey,
  HookHealth,
  HookStage,
  ImportPreview,
  ImportResolution,
  IpcError,
  PassphraseState,
  RememberOutcome,
  IpcErrorCode,
  Profile,
  ProfileSet,
  RepoRow,
  ScanProgress,
  Settings,
  SettingsPatch,
  ValidationIssue,
  WipeOptions,
  WipeReport,
} from './types';

const CODES: readonly IpcErrorCode[] = [
  'validation',
  'locked',
  'conflict',
  'not_found',
  'not_a_repo',
  'git',
  'io',
  'unsupported',
  'bad_input',
  'internal',
];

/** Normalise whatever `invoke` rejected with into an `IpcError`. */
export function toIpcError(e: unknown): IpcError {
  if (e && typeof e === 'object') {
    const o = e as Partial<IpcError> & { message?: unknown };
    const code = CODES.includes(o.code as IpcErrorCode) ? (o.code as IpcErrorCode) : 'internal';
    return {
      code,
      message: typeof o.message === 'string' && o.message ? o.message : 'Something went wrong.',
      issues: Array.isArray(o.issues) ? o.issues : [],
    };
  }
  if (typeof e === 'string' && e) return { code: 'internal', message: e, issues: [] };
  return { code: 'internal', message: 'Something went wrong.', issues: [] };
}

export function isIpcError(e: unknown): e is IpcError {
  return !!e && typeof e === 'object' && 'code' in e && 'message' in e && 'issues' in e;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toIpcError(e);
  }
}

// ---- State & profiles -------------------------------------------------------

export const getState = () => call<AppState>('get_state');
export const listProfiles = () => call<ProfileSet>('list_profiles');
export const validateProfile = (profile: Profile, originalId?: string) =>
  call<ValidationIssue[]>('validate_profile', { profile, originalId });
export const saveProfile = (profile: Profile, originalId?: string) =>
  call<ProfileSet>('save_profile', { profile, originalId });
export const deleteProfile = (id: string, newDefault?: string) =>
  call<ProfileSet>('delete_profile', { id, newDefault });
export const setDefault = (id: string) => call<ProfileSet>('set_default', { id });

// ---- Keys --------------------------------------------------------------------

export const suggestKeyPath = (id: string) => call<string>('suggest_key_path', { id });
/** Create (or reuse) the key and remember it; the passphrase is used once, in memory (ADR 0007). */
export const generateKey = (
  id: string,
  email: string,
  passphrase: string | null,
  noPassphrase: boolean,
) => call<GeneratedKey>('generate_key', { id, email, passphrase, noPassphrase });
export const readPublicKey = (id: string) => call<string>('read_public_key', { id });
/** Network (ssh -T) — only ever called from a user click. */
export const testConnection = (id: string) => call<ConnectionTest>('test_connection', { id });
/** Is the key's passphrase remembered by the Windows ssh-agent? (local only) */
export const passphraseState = (id: string) => call<PassphraseState>('passphrase_state', { id });
/**
 * Remember the key in the Windows ssh-agent. With `passphrase` (typed in GitHop,
 * ADR 0007) the backend unlocks the key in memory and never stores it; without
 * it, ssh-add runs in a visible console window instead.
 */
export const rememberPassphrase = (id: string, passphrase?: string) =>
  call<RememberOutcome>('remember_passphrase', { id, passphrase: passphrase ?? null });
/** One UAC prompt turns the ssh-agent service on, then remembers as above. */
export const turnOnAgent = (id: string, passphrase?: string) =>
  call<RememberOutcome>('turn_on_agent', { id, passphrase: passphrase ?? null });

// ---- Repos -------------------------------------------------------------------

export const scan = (folder: string) => call<RepoRow[]>('scan', { folder });
/** Stops a running scan in the backend (its results are dropped). */
export const cancelScan = () => call<void>('cancel_scan');
export const pin = (repo: string, id: string) => call<RepoRow>('pin', { repo, id });
export const unpin = (repo: string) => call<RepoRow>('unpin', { repo });
export const pinAll = (repos: string[]) => call<RepoRow[]>('pin_all', { repos });
export const hooksSnippet = (stage: HookStage) => call<string>('hooks_snippet', { stage });
/** Switch the repo's GitHub HTTPS remotes (or just `remote`) to SSH (ADR 0006). */
export const remoteSsh = (repo: string, remote?: string) =>
  call<RepoRow>('remote_ssh', { repo, remote });
export const remoteSshAll = (repos: string[]) => call<RepoRow[]>('remote_ssh_all', { repos });
/** Put back the HTTPS URLs GitHop switched. */
export const remoteUndo = (repo: string) => call<RepoRow>('remote_undo', { repo });

// ---- Fix ---------------------------------------------------------------------

export const fixPlan = (repo: string) => call<FixPlan>('fix_plan', { repo });
export const fixApply = (repo: string) => call<FixResult>('fix_apply', { repo });
export const fixUndo = (repo: string, backupRef: string) =>
  call<void>('fix_undo', { repo, backupRef });

// ---- Settings & system -------------------------------------------------------

export const doctor = () => call<DoctorReport>('doctor');
/** Writes the doctor report as plain text to `path` (from the native save dialog). */
export const saveDiagnostics = (path: string) => call<void>('save_diagnostics', { path });
export const updateSettings = (patch: SettingsPatch) =>
  call<Settings>('update_settings', { patch });
export const installHooks = () => call<HookHealth>('install_hooks');
export const wipe = (opts: WipeOptions) => call<WipeReport>('wipe', { opts });
export const exportProfiles = (path: string, password: string, includeKeys: boolean) =>
  call<void>('export_profiles', { path, password, includeKeys });
export const importPreview = (path: string, password: string) =>
  call<ImportPreview>('import_preview', { path, password });
export const importApply = (path: string, password: string, resolutions: ImportResolution[]) =>
  call<ProfileSet>('import_apply', { path, password, resolutions });
/** Opens the user's browser at a fixed, backend-scoped URL. GitHop itself sends nothing. */
export const openExternal = (target: ExternalTarget) => call<void>('open_external', { target });
export const appInfo = () => call<AppInfo>('app_info');

/** Design §8.7 "Restore last backup": newest valid backups/profiles.*.toml. */
export const restoreLastBackup = () => call<ProfileSet>('restore_profiles_backup');

// ---- Events ------------------------------------------------------------------

export const onProfilesChanged = (cb: () => void): Promise<UnlistenFn> =>
  listen('profiles://changed', () => cb());
export const onScanProgress = (cb: (p: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>('scan://progress', (e) => cb(e.payload));
export const onTrayDefaultChanged = (cb: (set: ProfileSet) => void): Promise<UnlistenFn> =>
  listen<ProfileSet>('tray://default-changed', (e) => cb(e.payload));

// ---- Native helpers (local only) ---------------------------------------------

/** Native folder picker. Returns null when cancelled. */
export async function pickFolder(title: string): Promise<string | null> {
  const r = await openDialog({ directory: true, multiple: false, title });
  return typeof r === 'string' ? r : null;
}

/** Native file picker (e.g. an existing private key). */
export async function pickFile(
  title: string,
  filters?: { name: string; extensions: string[] }[],
): Promise<string | null> {
  const r = await openDialog({ directory: false, multiple: false, title, filters });
  return typeof r === 'string' ? r : null;
}

/** Native save dialog (export file, diagnostics file). */
export async function pickSavePath(
  title: string,
  defaultPath: string,
  fileType: string,
  extensions: string[] = ['githop'],
): Promise<string | null> {
  const r = await saveDialog({
    title,
    defaultPath,
    filters: [{ name: fileType, extensions }],
  });
  return r ?? null;
}

export const copyText = (text: string) => writeText(text);

/** Show and focus the main window, hide the tray menu (tray window). */
export const showMainWindow = () => call<void>('show_main');

export const hideCurrentWindow = () => getCurrentWindow().hide();

/** Quit the whole app (tray menu). */
export const quitApp = () => call<void>('quit_app');

/** Hide the tray menu window when it loses focus (Design §7.14). */
export async function onCurrentWindowBlur(cb: () => void): Promise<UnlistenFn> {
  return getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (!focused) cb();
  });
}

/** Refetch triggers: window focus (Phase 5 §4.5). */
export async function onCurrentWindowFocus(cb: () => void): Promise<UnlistenFn> {
  return getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused) cb();
  });
}
