// "Test connection" results, kept per profile for this session only (memory,
// never written to disk), so the profile card and the edit form show the same
// last result. Testing runs `ssh -T` — only ever from a user click.
import * as ipc from './ipc';
import { createStore } from './query';
import type { ConnectionTest, IpcError } from './types';

export type ConnectionEntry =
  { status: 'testing' } | { status: 'done'; result: ConnectionTest; at: number };

export const connectionStore = createStore<Record<string, ConnectionEntry>>({});

/** Runs the test for `id` and records the result (an IPC error becomes `failed`). */
export async function runConnectionTest(id: string): Promise<ConnectionTest> {
  connectionStore.set((s) => ({ ...s, [id]: { status: 'testing' } }));
  let result: ConnectionTest;
  try {
    result = await ipc.testConnection(id);
  } catch (e) {
    result = { kind: 'failed', reason: (e as IpcError).message };
  }
  connectionStore.set((s) => ({ ...s, [id]: { status: 'done', result, at: Date.now() } }));
  return result;
}

/** Drop a profile's result (its key changed, or it was renamed or deleted). */
export function forgetConnectionTest(id: string): void {
  connectionStore.set((s) => {
    if (!(id in s)) return s;
    const next = { ...s };
    delete next[id];
    return next;
  });
}

export function useConnectionEntry(id: string | undefined): ConnectionEntry | undefined {
  const all = connectionStore.useStore();
  return id ? all[id] : undefined;
}

/**
 * The account GitHub signed in as isn't one of the profile's owners. Only when
 * the profile lists owners; compared case-insensitively.
 */
export function connectedElsewhere(user: string, owners: string[]): boolean {
  const u = user.trim().toLowerCase();
  const own = owners.map((o) => o.trim().toLowerCase()).filter(Boolean);
  return own.length > 0 && !own.includes(u);
}
