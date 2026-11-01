import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, beforeEach, vi } from 'vitest';

// Tauri modules must never be reached in tests: screens mock ../lib/ipc,
// and these stubs make any accidental direct use fail loudly.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.reject(new Error('invoke called without an ipc mock'))),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    hide: vi.fn(),
    destroy: vi.fn(),
    onFocusChanged: vi.fn(() => Promise.resolve(() => undefined)),
  }),
  Window: { getByLabel: vi.fn(() => Promise.resolve(null)) },
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn() }));

// axe probes canvas for icon ligatures; jsdom has none.
HTMLCanvasElement.prototype.getContext = (() => null) as never;

// Imported lazily so each test file's vi.mock('../lib/ipc') applies to them.
beforeEach(async () => {
  const { resetQueries } = await import('../lib/query');
  const { reposStore } = await import('../lib/appState');
  const { tooltipGroup } = await import('../lib/motion');
  const { connectionStore } = await import('../lib/connection');
  resetQueries();
  reposStore.reset();
  tooltipGroup.reset();
  connectionStore.reset();
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
});
