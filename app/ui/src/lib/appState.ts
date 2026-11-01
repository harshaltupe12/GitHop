import { normalizePath } from './format';
import * as ipc from './ipc';
import { createStore, invalidate, setQueryData, useQuery } from './query';
import type { AppState, ProfileSet, RepoRow } from './types';

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

export type ReposState = {
  folders: string[];
  /** Rows carry `unpushed_offenders` (IPC.md), so no per-repo fix_plan is needed. */
  repos: RepoRow[];
  scanning: { folder: string; found: number } | null;
  scanned: boolean;
  autoScanned: boolean;
};

export const reposStore = createStore<ReposState>({
  folders: [],
  repos: [],
  scanning: null,
  scanned: false,
  autoScanned: false,
});

/** Paths compare with forward slashes, case-insensitively (Windows). */
export const sameRoot = (a: string, b: string) =>
  normalizePath(a).toLowerCase() === normalizePath(b).toLowerCase();

export function replaceRepo(updated: RepoRow): void {
  reposStore.set((s) => ({
    ...s,
    repos: s.repos.map((r) => (sameRoot(r.root, updated.root) ? updated : r)),
  }));
}

/** After a fresh fix plan: keep the Repos pill in step with what Fix can change. */
export function setUnpushedOffenders(root: string, n: number): void {
  reposStore.set((s) => ({
    ...s,
    repos: s.repos.map((r) => (sameRoot(r.root, root) ? { ...r, unpushed_offenders: n } : r)),
  }));
}
