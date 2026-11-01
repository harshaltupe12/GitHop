import { useId, useMemo, useState } from 'react';
import { Avatar } from '../components/Avatar';
import { Banner } from '../components/Banner';
import { Button } from '../components/Button';
import { Ico, Icons } from '../components/icons';
import { useToast } from '../components/Toast';
import { applyProfiles } from '../lib/appState';
import { defaultProfile, profileById, sortProfiles } from '../lib/format';
import * as ipc from '../lib/ipc';
import { ownerGroups, suggestProfile, type OwnerGroup } from '../lib/owners';
import type { IpcError, ProfileSet, RepoRow } from '../lib/types';
import { S } from '../strings';

const FIRST = 5;

type Added = { owner: string; label: string };

/**
 * "Owners found": scan results that only the default profile picked, grouped
 * by GitHub owner, with a profile picker per owner. Nothing is applied until
 * the user clicks "Add to profile" (a suggestion is only pre-selected).
 */
export function OwnersPanel({
  rows,
  profiles,
  scanning,
  onAdded,
  pinCount,
  pinning,
  onPinAll,
}: {
  rows: RepoRow[];
  profiles: ProfileSet;
  scanning: boolean;
  /** Refresh the rows after an owner was added (rescan). */
  onAdded: () => void;
  pinCount: number;
  pinning: boolean;
  onPinAll: () => void;
}) {
  const titleId = useId();
  const [added, setAdded] = useState<Added | null>(null);
  // Owners added this session: hidden at once, before the profiles refetch lands.
  const [done, setDone] = useState<Set<string>>(new Set());
  const [showAll, setShowAll] = useState(false);

  const groups = useMemo(
    () => ownerGroups(rows, profiles).filter((g) => !done.has(g.owner)),
    [rows, profiles, done],
  );
  if (!groups.length && !added) return null;

  const total = groups.reduce((n, g) => n + g.count, 0);
  const def = defaultProfile(profiles);
  const shown = showAll ? groups : groups.slice(0, FIRST);
  const hidden = groups.length - shown.length;

  return (
    <section
      aria-labelledby={titleId}
      className="appear mb-4 flex flex-col gap-3 rounded-lg border border-line bg-surface px-4 py-3"
    >
      {added ? (
        <Banner
          kind="success"
          action={
            !scanning && pinCount > 0
              ? { label: S.repos.pinAll, onClick: onPinAll, busy: pinning }
              : undefined
          }
        >
          <p>{S.repos.owners.addedBody(added.owner, added.label)}</p>
          <p>{scanning ? S.repos.owners.updating : pinCount > 0 ? S.repos.owners.pinHint : null}</p>
        </Banner>
      ) : null}
      {groups.length ? (
        <>
          <div className="flex items-start gap-2">
            <Ico icon={Icons.warning} className="mt-1 text-warning-ink" />
            <div className="min-w-0 flex-1">
              <h2 id={titleId} className="text-heading">
                {S.repos.owners.title}
              </h2>
              <p className="mt-1 text-body text-ink-muted">
                {S.repos.owners.body(total, def?.label ?? null)}
              </p>
            </div>
          </div>
          <ul
            aria-label={S.repos.owners.listLabel}
            className="flex flex-col divide-y divide-line-row"
          >
            {shown.map((g) => (
              <OwnerRow
                key={g.owner}
                group={g}
                profiles={profiles}
                onAdded={(label) => {
                  setDone((s) => new Set(s).add(g.owner));
                  setAdded({ owner: g.owner, label });
                  onAdded();
                }}
              />
            ))}
          </ul>
          {groups.length > FIRST ? (
            <div>
              <Button variant="ghost" compact onClick={() => setShowAll((v) => !v)}>
                {hidden > 0 ? S.repos.owners.showMore(hidden) : S.repos.owners.showFewer}
              </Button>
            </div>
          ) : null}
        </>
      ) : (
        <h2 id={titleId} className="sr-only">
          {S.repos.owners.title}
        </h2>
      )}
    </section>
  );
}

function OwnerRow({
  group,
  profiles,
  onAdded,
}: {
  group: OwnerGroup;
  profiles: ProfileSet;
  onAdded: (label: string) => void;
}) {
  const toast = useToast();
  const selectId = useId();
  const errorId = `${selectId}-error`;
  const suggestion = useMemo(() => suggestProfile(group.owner, profiles), [group.owner, profiles]);
  const [choice, setChoice] = useState(suggestion?.id ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ordered = sortProfiles(profiles);
  const chosen = profileById(profiles, choice);

  const add = async () => {
    if (!choice) return;
    setBusy(true);
    setError(null);
    try {
      // Start from the stored profile, not a possibly stale copy.
      const fresh = await ipc.listProfiles();
      const p = fresh.profile.find((x) => x.id === choice);
      if (!p) {
        // Deleted elsewhere since this screen loaded.
        applyProfiles(fresh);
        setChoice('');
        setError(S.repos.owners.gone);
        return;
      }
      const owners = p.owners.some((o) => o.trim().toLowerCase() === group.owner)
        ? p.owners
        : [...p.owners, group.owner];
      const set = await ipc.saveProfile({ ...p, owners }, p.id);
      applyProfiles(set);
      toast({ message: S.repos.owners.added(group.owner, p.label) });
      onAdded(p.label);
    } catch (e) {
      const ie = e as IpcError;
      // The core refuses an owner another profile already has; show its words.
      const own = ie.issues?.filter((i) => i.severity === 'error').map((i) => i.message) ?? [];
      setError(own.length ? own.join(' ') : ie.message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <li className="flex flex-col gap-1 py-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-[10rem] flex-1">
          <span className="block truncate font-mono text-mono text-ink" title={group.owner}>
            {group.owner}
          </span>
          <span className="block text-small text-ink-muted">
            {S.repos.owners.repoCount(group.count)}
          </span>
        </span>
        <span className="flex items-center gap-2">
          {chosen ? (
            <Avatar label={chosen.label} color={chosen.color} size={22} decorative />
          ) : (
            <span
              aria-hidden="true"
              className="h-[1.5714rem] w-[1.5714rem] rounded-full border border-dashed border-line-input"
            />
          )}
          <label htmlFor={selectId} className="sr-only">
            {S.repos.owners.profileFor(group.owner)}
          </label>
          <select
            id={selectId}
            value={choice}
            onChange={(e) => {
              setChoice(e.target.value);
              setError(null);
            }}
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? errorId : undefined}
            className="gh-input min-h-8 rounded-md border border-line-input bg-surface px-2 text-body text-ink"
          >
            {suggestion ? null : (
              <option value="" disabled>
                {S.repos.owners.choose}
              </option>
            )}
            {ordered.map((p) => (
              <option key={p.id} value={p.id}>
                {p.id === suggestion?.id ? S.repos.owners.suggested(p.label) : p.label}
              </option>
            ))}
          </select>
          <Button
            icon={Icons.add}
            onClick={() => void add()}
            loading={busy}
            disabled={!choice}
            aria-label={S.repos.owners.addLabel(group.owner)}
          >
            {S.repos.owners.add}
          </Button>
        </span>
      </div>
      {error ? (
        <p
          id={errorId}
          role="alert"
          className="flex items-start gap-1 text-small text-blocked-text"
        >
          <Ico icon={Icons.warning} className="mt-px" />
          {error}
        </p>
      ) : null}
    </li>
  );
}
