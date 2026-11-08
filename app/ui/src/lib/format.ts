import { domainStem } from './owners';
import { PROFILE_COLORS, type Profile, type ProfileColor, type ProfileSet } from './types';

/** `C:/Users/jane/code/githop` → `C:/Users/…/githop` when longer than `max`. */
export function truncateMiddle(path: string, max = 36): string {
  if (path.length <= max) return path;
  const p = path.replace(/\\/g, '/');
  const parts = p.split('/').filter(Boolean);
  if (parts.length >= 3) {
    const head = parts[0];
    const tail = parts[parts.length - 1];
    const candidate = `${head}/${parts[1]}/…/${tail}`;
    if (candidate.length <= max) return candidate;
    const short = `${head}/…/${tail}`;
    if (short.length <= max) return short;
  }
  const keep = Math.max(4, Math.floor((max - 1) / 2));
  return `${p.slice(0, keep)}…${p.slice(-keep)}`;
}

/** Forward slashes, as the core normalises them. */
export const normalizePath = (p: string) => p.replace(/\\/g, '/');

/** Last path segment: repo folder name. */
export function baseName(path: string): string {
  const parts = normalizePath(path).split('/').filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export const shortSha = (sha: string) => sha.slice(0, 7);

/** Profile id slug: `^[a-z0-9-]{1,32}$`. */
export function slugify(label: string): string {
  return label
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 32)
    .replace(/-+$/g, '');
}

/** Default profile first, then alphabetical by label (Design §8.1). */
export function sortProfiles(set: ProfileSet): Profile[] {
  const def = set.default ?? null;
  return [...set.profile].sort((a, b) => {
    if (a.id === def) return -1;
    if (b.id === def) return 1;
    return a.label.localeCompare(b.label, undefined, { sensitivity: 'base' });
  });
}

export function defaultProfile(set: ProfileSet | undefined): Profile | undefined {
  if (!set?.default) return undefined;
  return set.profile.find((p) => p.id === set.default);
}

export function profileById(set: ProfileSet | undefined, id?: string | null): Profile | undefined {
  if (!set || !id) return undefined;
  return set.profile.find((p) => p.id === id);
}

export function profileByEmail(set: ProfileSet | undefined, email: string): Profile | undefined {
  const e = email.trim().toLowerCase();
  return set?.profile.find((p) => p.email.trim().toLowerCase() === e);
}

/** Next unused colour (Design §2.2: new profiles get the next unused colour). */
export function nextColor(set: ProfileSet | undefined, exceptId?: string): ProfileColor {
  const used = new Set(set?.profile.filter((p) => p.id !== exceptId).map((p) => p.color) ?? []);
  return PROFILE_COLORS.find((c) => !used.has(c)) ?? 'blue';
}

export function initial(label: string): string {
  const ch = label.trim().charAt(0);
  return ch ? ch.toUpperCase() : '?';
}

/** `C:/Users/jane/.ssh/githop_work` → `githop_work`. */
export function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/**
 * Onboarding: a label suggestion for the git identity already on the PC, always
 * editable. `jane@acmelabs.com` → `Acmelabs`; free-mail or GitHub noreply →
 * `Personal`; nothing usable → `''` (the user names it).
 */
export function suggestLabel(email: string | undefined, personal: string): string {
  const e = email?.trim() ?? '';
  if (!e.includes('@')) return '';
  const stem = domainStem(e);
  return stem ? stem.charAt(0).toUpperCase() + stem.slice(1) : personal;
}

export const NOREPLY_TEMPLATE = (owner: string) =>
  `ID+${owner || 'username'}@users.noreply.github.com`;

/** Rough password strength for the export dialog hint (no dictionary, local only). */
export function passwordStrength(pw: string): 0 | 1 | 2 {
  if (pw.length < 12) return 0;
  let classes = 0;
  if (/[a-z]/.test(pw)) classes++;
  if (/[A-Z]/.test(pw)) classes++;
  if (/[0-9]/.test(pw)) classes++;
  if (/[^A-Za-z0-9]/.test(pw)) classes++;
  if (pw.length >= 16 && classes >= 3) return 2;
  if (pw.length >= 20) return 2;
  return 1;
}
