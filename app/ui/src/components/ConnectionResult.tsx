import { useEffect, useState } from 'react';
import {
  connectedElsewhere,
  runConnectionTest,
  useConnectionEntry,
  type ConnectionEntry,
} from '../lib/connection';
import { cx } from '../lib/cx';
import * as ipc from '../lib/ipc';
import type { IpcError, Profile } from '../lib/types';
import { S } from '../strings';
import { Banner } from './Banner';
import { Button } from './Button';
import { Spinner } from './Spinner';
import { useToast } from './Toast';
import { Icons } from './icons';

/** Runs "Test connection" for a profile: busy state + result in the session store, toast on success. */
export function useTestConnection() {
  const toast = useToast();
  return async (p: Pick<Profile, 'id'>) => {
    const r = await runConnectionTest(p.id);
    if (r.kind === 'connected') toast({ message: S.profiles.connected(r.user) });
  };
}

export function ago(at: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 60) return S.connection.justNow;
  const m = Math.round(s / 60);
  if (m < 60) return S.connection.minutesAgo(m);
  return S.connection.at(
    new Date(at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' }),
  );
}

/** Re-render every 30 s so "just now" turns into "1 min ago". */
function useTick(active: boolean) {
  const [, setN] = useState(0);
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setN((n) => n + 1), 30_000);
    return () => window.clearInterval(t);
  }, [active]);
}

/**
 * The last "Test connection" result for a profile, persistent until the next
 * test (session only). Success: check + account + time; denied: what to do next
 * with Copy public key / Open GitHub SSH settings; failed: the reason + Try again.
 * Warns when GitHub signed in as an account the profile doesn't own.
 */
export function ConnectionResult({
  profile,
  publicKey,
  showTesting = true,
  actions = true,
  className,
}: {
  profile: Pick<Profile, 'id' | 'owners'>;
  /** Already-loaded public key (edit form); otherwise read on Copy. */
  publicKey?: string | null;
  /** Show the inline "Testing…" line (off where the button already shows it). */
  showTesting?: boolean;
  /** Show Copy public key / Open GitHub SSH settings on "denied" (off where they're already next to it). */
  actions?: boolean;
  className?: string;
}) {
  const entry = useConnectionEntry(profile.id);
  useTick(entry?.status === 'done');
  if (!entry) return null;
  if (entry.status === 'testing') {
    return showTesting ? (
      <p role="status" className={cx('flex items-center gap-2 text-small text-ink-muted', className)}>
        <Spinner />
        {S.profiles.testing}
      </p>
    ) : null;
  }
  return (
    <Result
      entry={entry}
      profile={profile}
      publicKey={publicKey}
      actions={actions}
      className={className}
    />
  );
}

function Result({
  entry,
  profile,
  publicKey,
  actions,
  className,
}: {
  entry: Extract<ConnectionEntry, { status: 'done' }>;
  profile: Pick<Profile, 'id' | 'owners'>;
  publicKey?: string | null;
  actions: boolean;
  className?: string;
}) {
  const toast = useToast();
  const test = useTestConnection();
  const r = entry.result;
  const owners = profile.owners.filter((o) => o.trim());

  const copy = async () => {
    try {
      const key = publicKey ?? (await ipc.readPublicKey(profile.id)).trim();
      await ipc.copyText(key);
      toast({ message: S.form.keyCopied });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message || String(e) });
    }
  };

  if (r.kind === 'connected') {
    const elsewhere = connectedElsewhere(r.user, owners);
    return (
      <div className={cx('flex flex-col gap-2', className)}>
        <Banner kind="success">
          <span className="font-medium">{S.connection.connected(r.user)}</span>
          <span> · {ago(entry.at)}</span>
        </Banner>
        {elsewhere ? (
          <Banner kind="warning">
            <p className="font-medium">{S.connection.elsewhere(r.user, owners)}</p>
            <p>{S.connection.elsewhereHelp(r.user)}</p>
          </Banner>
        ) : null}
      </div>
    );
  }
  if (r.kind === 'denied') {
    return (
      <Banner kind="warning" className={className}>
        <p className="font-medium">{S.connection.denied}</p>
        <p>{S.connection.deniedHelp(owners)}</p>
        {actions ? (
        <div className="mt-2 flex flex-wrap gap-2">
          <Button compact icon={Icons.copy} onClick={() => void copy()}>
            {S.connection.copyKey}
          </Button>
          <Button compact icon={Icons.external} onClick={() => void ipc.openExternal('ssh-keys')}>
            {S.connection.openGithub}
          </Button>
        </div>
        ) : null}
      </Banner>
    );
  }
  return (
    <Banner
      kind="blocked"
      className={className}
      action={{ label: S.connection.retry, onClick: () => void test(profile) }}
    >
      {S.connection.failed(r.reason)}
    </Banner>
  );
}
