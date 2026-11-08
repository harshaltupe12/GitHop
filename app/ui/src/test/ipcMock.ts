import { vi } from 'vitest';
import { appState, plan, profileSet } from './fixtures';

const noop = () => undefined;

/** Full stand-in for src/lib/ipc.ts. Every function is a vi.fn with a sensible default. */
export async function createIpcMock() {
  const real = await vi.importActual<typeof import('../lib/ipc')>('../lib/ipc');
  return {
    toIpcError: real.toIpcError,
    isIpcError: real.isIpcError,
    getState: vi.fn(() => Promise.resolve(appState())),
    listProfiles: vi.fn(() => Promise.resolve(profileSet())),
    validateProfile: vi.fn(() => Promise.resolve([])),
    saveProfile: vi.fn(() => Promise.resolve(profileSet())),
    deleteProfile: vi.fn(() => Promise.resolve(profileSet())),
    setDefault: vi.fn((id: string) => Promise.resolve(profileSet({ default: id }))),
    suggestKeyPath: vi.fn((id: string) => Promise.resolve(`C:/Users/jane/.ssh/githop_${id}`)),
    generateKey: vi.fn((id: string) =>
      Promise.resolve({
        public_key: 'ssh-ed25519 AAAAC3Nza githop',
        path: `C:/Users/jane/.ssh/githop_${id}`,
        reused: false,
      }),
    ),
    readPublicKey: vi.fn(() =>
      Promise.reject({ code: 'not_found', message: 'No public key.', issues: [] }),
    ),
    testConnection: vi.fn(() => Promise.resolve({ kind: 'connected', user: 'jane' })),
    passphraseState: vi.fn(() => Promise.resolve('asks-each-time')),
    rememberPassphrase: vi.fn(() => Promise.resolve('remembered')),
    turnOnAgent: vi.fn(() => Promise.resolve('remembered')),
    remoteSsh: vi.fn(),
    remoteSshAll: vi.fn(() => Promise.resolve([])),
    remoteUndo: vi.fn(),
    scan: vi.fn(() => Promise.resolve([])),
    cancelScan: vi.fn(() => Promise.resolve()),
    pin: vi.fn(),
    unpin: vi.fn(),
    pinAll: vi.fn(() => Promise.resolve([])),
    hooksSnippet: vi.fn(() => Promise.resolve('githop check commit || exit $?')),
    fixPlan: vi.fn(() => Promise.resolve(plan({ offenders: [] }))),
    fixApply: vi.fn(() =>
      Promise.resolve({ backup_ref: 'refs/githop/backup/1', rewritten: 2, undo_hint: '' }),
    ),
    fixUndo: vi.fn(() => Promise.resolve()),
    doctor: vi.fn(() => Promise.resolve({ checks: [] })),
    saveDiagnostics: vi.fn(() => Promise.resolve()),
    updateSettings: vi.fn(() => Promise.resolve(appState().settings)),
    installHooks: vi.fn(() => Promise.resolve(appState().hooks)),
    wipe: vi.fn(() => Promise.resolve({ steps: [] })),
    exportProfiles: vi.fn(() => Promise.resolve()),
    importPreview: vi.fn(),
    importApply: vi.fn(() => Promise.resolve(profileSet())),
    openExternal: vi.fn(() => Promise.resolve()),
    appInfo: vi.fn(() => Promise.resolve({ version: '0.1.0', licenses: 'IBM Plex — SIL OFL 1.1' })),
    restoreLastBackup: vi.fn(() => Promise.resolve(profileSet())),
    onProfilesChanged: vi.fn(() => Promise.resolve(noop)),
    onScanProgress: vi.fn(() => Promise.resolve(noop)),
    onTrayDefaultChanged: vi.fn(() => Promise.resolve(noop)),
    pickFolder: vi.fn(() => Promise.resolve(null)),
    pickFile: vi.fn(() => Promise.resolve(null)),
    pickSavePath: vi.fn(() => Promise.resolve(null)),
    copyText: vi.fn(() => Promise.resolve()),
    showMainWindow: vi.fn(() => Promise.resolve()),
    hideCurrentWindow: vi.fn(() => Promise.resolve()),
    quitApp: vi.fn(() => Promise.resolve()),
    onCurrentWindowBlur: vi.fn(() => Promise.resolve(noop)),
    onCurrentWindowFocus: vi.fn(() => Promise.resolve(noop)),
  };
}
