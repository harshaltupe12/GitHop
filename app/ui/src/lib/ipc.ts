// Typed wrappers for every backend call (app/IPC.md). The only module that
// imports @tauri-apps/* — components and screens call these functions.
// Nothing here makes a network request: `invoke` goes over Tauri's local IPC.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow, Window } from '@tauri-apps/api/window';
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
  HookHealth,
  HookStage,
  ImportPreview,
  ImportResolution,
  IpcError,
  IpcErrorCode,
  Profile,
  ProfileSet,
  ScannedRepo,
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
export const generateKey = (id: string, noPassphrase: boolean) =>
  call<string>('generate_key', { id, noPassphrase });
export const readPublicKey = (id: string) => call<string>('read_public_key', { id });
/** Network (ssh -T) — only ever called from a user click. */
export const testConnection = (id: string) => call<ConnectionTest>('test_connection', { id });

// ---- Repos -------------------------------------------------------------------

export const scan = (folder: string) => call<ScannedRepo[]>('scan', { folder });
export const pin = (repo: string, id: string) => call<ScannedRepo>('pin', { repo, id });
export const unpin = (repo: string) => call<ScannedRepo>('unpin', { repo });
export const pinAll = (repos: string[]) => call<ScannedRepo[]>('pin_all', { repos });
export const hooksSnippet = (stage: HookStage) => call<string>('hooks_snippet', { stage });

// ---- Fix ---------------------------------------------------------------------

export const fixPlan = (repo: string) => call<FixPlan>('fix_plan', { repo });
export const fixApply = (repo: string) => call<FixResult>('fix_apply', { repo });
export const fixUndo = (repo: string, backupRef: string) =>
  call<void>('fix_undo', { repo, backupRef });

// ---- Settings & system -------------------------------------------------------

export const doctor = () => call<DoctorReport>('doctor');
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

// Not in app/IPC.md yet — proposed additions (see report). They fail with an
// IpcError until the backend registers them, which the UI shows as a message.
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

/** Native save dialog (export file). */
export async function pickSavePath(
  title: string,
  defaultPath: string,
  fileType: string,
): Promise<string | null> {
  const r = await saveDialog({
    title,
    defaultPath,
    filters: [{ name: fileType, extensions: ['githop'] }],
  });
  return r ?? null;
}

export const copyText = (text: string) => writeText(text);

/** Show and focus the main window (from the tray menu window). */
export async function showMainWindow(): Promise<void> {
  try {
    await invoke('show_main');
    return;
  } catch {
    // Proposed command missing → use the window API directly.
  }
  const main = await Window.getByLabel('main');
  if (main) {
    await main.show();
    await main.unminimize();
    await main.setFocus();
  }
  await hideCurrentWindow();
}

export const hideCurrentWindow = () => getCurrentWindow().hide();

/** Quit the whole app (tray menu). Needs the proposed `quit_app` command. */
export async function quitApp(): Promise<void> {
  try {
    await invoke('quit_app');
  } catch {
    // Fallback: close every window; the backend exits when none remain.
    await getCurrentWindow().destroy();
  }
}

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
