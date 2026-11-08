// Owner routing helpers for the Repos "owners found" panel and the default-only
// row wording. Mirrors the CLI (`githop status` / `githop scan` hints).
import type { Profile, ProfileSet, RepoRow } from './types';

const norm = (o: string) => o.trim().toLowerCase();

/**
 * The GitHub owner of a repo that no rule matched: it uses the default profile
 * (or nothing), it isn't pinned, and its remote is on GitHub. `null` otherwise.
 */
export function unmatchedOwner(r: RepoRow): string | null {
  if (r.error) return null;
  const res = r.resolution;
  if (res.source && res.source !== 'default') return null;
  if (res.pinned) return null;
  if (res.warnings.some((w) => w.kind === 'non-github-host')) return null;
  const owner = res.remote?.owner?.trim();
  return owner ? norm(owner) : null;
}

/** The profile that already lists `owner` (case-insensitive), if any. */
export function ownerClaimedBy(set: ProfileSet, owner: string): Profile | undefined {
  const o = norm(owner);
  return set.profile.find((p) => p.owners.some((x) => norm(x) === o));
}

export type OwnerGroup = { owner: string; count: number; roots: string[] };

/**
 * Default-only repos grouped by GitHub owner, most repos first. Owners a profile
 * already claims are left out (they route on the next scan).
 */
export function ownerGroups(rows: RepoRow[], set: ProfileSet): OwnerGroup[] {
  const m = new Map<string, OwnerGroup>();
  for (const r of rows) {
    const owner = unmatchedOwner(r);
    if (!owner || ownerClaimedBy(set, owner)) continue;
    const g = m.get(owner) ?? { owner, count: 0, roots: [] };
    g.count++;
    g.roots.push(r.root);
    m.set(owner, g);
  }
  return [...m.values()].sort((a, b) => b.count - a.count || a.owner.localeCompare(b.owner));
}

/** Free-mail and GitHub noreply domains say nothing about who owns a repo. */
const GENERIC_DOMAIN =
  /^(users\.noreply\.github\.com|gmail|googlemail|outlook|hotmail|live|yahoo|icloud|me|proton|protonmail|pm|gmx|aol|yandex|zoho|fastmail)(\.|$)/i;

/** `jane@acmelabs.com` → `acmelabs`; `null` for free-mail and noreply. */
export function domainStem(email: string): string | null {
  const domain = email.trim().toLowerCase().split('@')[1];
  if (!domain || GENERIC_DOMAIN.test(domain)) return null;
  const parts = domain.split('.').filter(Boolean);
  // `mail.company.co.uk` → `company`: the longest label before the public suffix.
  const labels = parts.length > 1 ? parts.slice(0, -1) : parts;
  const stem = labels
    .filter((l) => !['co', 'com', 'org', 'net', 'ac', 'gov', 'mail'].includes(l))
    .sort((a, b) => b.length - a.length)[0];
  return stem && stem.length >= 3 ? stem.replace(/[^a-z0-9]/g, '') : null;
}

/**
 * A suggestion only, never applied on its own: the profile whose email domain
 * matches the owner (`acme-labs` / `jane-acme` ↔ `@acmelabs.com`).
 * A word of 4+ letters in the owner that appears in the stem (or the reverse) counts.
 */
export function suggestProfile(owner: string, set: ProfileSet): Profile | undefined {
  const o = norm(owner);
  const flat = o.replace(/[^a-z0-9]/g, '');
  const words = o.split(/[^a-z0-9]+/).filter((w) => w.length >= 4);
  const hits = set.profile.filter((p) => {
    const stem = domainStem(p.email);
    if (!stem) return false;
    return flat.includes(stem) || words.some((w) => stem.includes(w));
  });
  return hits.length === 1 ? hits[0] : undefined;
}

/**
 * Non-default profiles with no owners, when there are two or more profiles. An
 * owner rule never picks them, so their repos quietly use the default profile.
 */
export function ownerlessProfiles(set: ProfileSet): Profile[] {
  if (set.profile.length < 2) return [];
  return set.profile.filter((p) => p.id !== set.default && p.owners.every((o) => !o.trim()));
}
