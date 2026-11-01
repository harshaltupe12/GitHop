// Tiny, failure-tolerant localStorage helpers (UI conveniences only — never
// data GitHop depends on; profiles and settings live in the backend).
export function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(key);
    return raw == null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

export function writeJson(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage full or blocked: the UI keeps working without it.
  }
}

export function removeKey(key: string): void {
  try {
    window.localStorage.removeItem(key);
  } catch {
    // ignore
  }
}

export const KEYS = {
  scanFolders: 'githop.repos.folders',
  onboardingStep: 'githop.onboarding.step',
  onboardingDone: 'githop.onboarding.done',
} as const;
