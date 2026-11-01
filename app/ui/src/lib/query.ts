// A tiny query layer over ipc.ts: cached by key, de-duplicated in flight,
// refetched on window focus and on backend change events (wired in App).
import { useCallback, useEffect, useSyncExternalStore } from 'react';
import { toIpcError } from './ipc';
import type { IpcError } from './types';

type Entry = {
  data?: unknown;
  error?: IpcError;
  loading: boolean;
  stale: boolean;
  inflight?: Promise<unknown>;
  fetcher?: () => Promise<unknown>;
  listeners: Set<() => void>;
  snapshot: Snapshot<unknown>;
};

export type Snapshot<T> = {
  data: T | undefined;
  error: IpcError | undefined;
  loading: boolean;
};

const cache = new Map<string, Entry>();

function entry(key: string): Entry {
  let e = cache.get(key);
  if (!e) {
    e = {
      loading: false,
      stale: true,
      listeners: new Set(),
      snapshot: { data: undefined, error: undefined, loading: false },
    };
    cache.set(key, e);
  }
  return e;
}

function emit(e: Entry) {
  e.snapshot = { data: e.data, error: e.error, loading: e.loading };
  e.listeners.forEach((l) => l());
}

function run(key: string): Promise<unknown> | undefined {
  const e = entry(key);
  if (!e.fetcher) return undefined;
  if (e.inflight) return e.inflight;
  e.loading = true;
  emit(e);
  const p = e
    .fetcher()
    .then(
      (d) => {
        e.data = d;
        e.error = undefined;
        e.stale = false;
      },
      (err) => {
        e.error = toIpcError(err);
      },
    )
    .finally(() => {
      e.inflight = undefined;
      e.loading = false;
      emit(e);
    });
  e.inflight = p;
  return p;
}

export function useQuery<T>(key: string, fetcher: () => Promise<T>) {
  const e = entry(key);
  e.fetcher = fetcher as () => Promise<unknown>;
  const subscribe = useCallback(
    (l: () => void) => {
      const en = entry(key);
      en.listeners.add(l);
      return () => {
        en.listeners.delete(l);
      };
    },
    [key],
  );
  const snap = useSyncExternalStore(subscribe, () => entry(key).snapshot) as Snapshot<T>;
  useEffect(() => {
    const en = entry(key);
    if (en.stale && !en.inflight) void run(key);
  }, [key]);
  const refetch = useCallback(() => run(key), [key]);
  return { ...snap, refetch };
}

/** Mark stale and refetch if anything is watching (all keys when omitted). */
export function invalidate(key?: string): void {
  const keys = key ? [key] : [...cache.keys()];
  for (const k of keys) {
    const e = entry(k);
    e.stale = true;
    if (e.listeners.size > 0) void run(k);
  }
}

export function setQueryData<T>(key: string, update: T | ((old: T | undefined) => T)): void {
  const e = entry(key);
  e.data =
    typeof update === 'function'
      ? (update as (o: T | undefined) => T)(e.data as T | undefined)
      : update;
  e.error = undefined;
  e.stale = false;
  emit(e);
}

export function getQueryData<T>(key: string): T | undefined {
  return cache.get(key)?.data as T | undefined;
}

/** Tests only. */
export function resetQueries(): void {
  cache.clear();
}

// ---- A minimal shared store (scan results shared by Repos and Fix) ----------

export function createStore<T>(initial: T) {
  let value = initial;
  const listeners = new Set<() => void>();
  const get = () => value;
  const set = (next: T | ((old: T) => T)) => {
    value = typeof next === 'function' ? (next as (o: T) => T)(value) : next;
    listeners.forEach((l) => l());
  };
  const subscribe = (l: () => void) => {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  };
  const reset = () => set(initial);
  function useStore(): T {
    return useSyncExternalStore(subscribe, get);
  }
  return { get, set, subscribe, reset, useStore };
}
