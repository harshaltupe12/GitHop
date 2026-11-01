import * as ipc from './ipc';
import { createStore, invalidate, setQueryData, useQuery } from './query';
import type { AppState, ProfileSet, ScannedRepo } from './types';

export const STATE_KEY = 'state';

export function useAppState() {
  return useQuery<AppState>(STATE_KEY, ipc.getState);
}

/** Apply a ProfileSet returned by a mutation, then refetch the rest of the state. */
export function applyProfiles(set: ProfileSet): void {
  setQueryData<AppState | undefined>(STATE_KEY, (s) => (s ? { ...s, profiles: set } : s));
  invalidate(STATE_KEY);
}

export function hooksHealthy(s: AppState): boolean {
  const h = s.hooks;
  return h.installed && h.points_to_us && h.missing.length === 0 && !h.exe_missing;
}

// ---- Repos scan results (shared by Repos and Fix screens) --------------------

export type OffenderCount = number | 'checking' | 'error';

export type ReposState = {
  folders: string[];
  repos: ScannedRepo[];
  offenders: Record<string, OffenderCount>;
  scanning: { folder: string; found: number } | null;
  scanned: boolean;
  autoScanned: boolean;
};

export const reposStore = createStore<ReposState>({
  folders: [],
  repos: [],
  offenders: {},
  scanning: null,
  scanned: false,
  autoScanned: false,
});

export function replaceRepo(updated: ScannedRepo): void {
  reposStore.set((s) => ({
    ...s,
    repos: s.repos.map((r) => (r.root === updated.root ? updated : r)),
  }));
}
