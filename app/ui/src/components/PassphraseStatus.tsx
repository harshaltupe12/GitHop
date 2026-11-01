import { useCallback, useEffect, useState } from 'react';
import { cx } from '../lib/cx';
import * as ipc from '../lib/ipc';
import type { IpcError, PassphraseState, RememberOutcome } from '../lib/types';
import { S } from '../strings';
import { Button } from './Button';
import { Ico, Icons } from './icons';
import { useToast } from './Toast';

/**
 * Key panel line (ADR 0006): does the Windows ssh-agent remember this key's
 * passphrase? Status is icon + text (Design §11). "Remember my passphrase"
 * runs ssh-add in a visible console window (the passphrase is typed there,
 * never in GitHop); with the agent off, "Turn on and remember" asks Windows
 * for admin rights once (UAC) first. VS Code's Git has no terminal to ask for
 * a passphrase, so this is what makes its pushes work.
 */
export function PassphraseStatus({ profileId, keyPath }: { profileId: string; keyPath: string }) {
  const toast = useToast();
  const [state, setState] = useState<PassphraseState | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    let live = true;
    ipc
      .passphraseState(profileId)
      .then((s) => live && setState(s))
      .catch(() => live && setState(null));
    return () => {
      live = false;
    };
  }, [profileId]);

  useEffect(() => load(), [load, keyPath]);

  const run = async (turnOn: boolean) => {
    setBusy(true);
    try {
      const out: RememberOutcome = turnOn
        ? await ipc.turnOnAgent(profileId)
        : await ipc.rememberPassphrase(profileId);
      if (out === 'remembered') {
        setState('remembered');
        toast({ message: S.passphrase.done });
      } else if (out === 'no-passphrase') {
        setState('no-passphrase');
      } else {
        const message =
          out === 'cancelled'
            ? S.passphrase.cancelled
            : out === 'agent-off'
              ? S.passphrase.stillOff
              : S.passphrase.notAdded;
        toast({ kind: 'warning', message });
        load();
      }
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setBusy(false);
    }
  };

  if (!state || state === 'no-key' || state === 'unknown') return null;

  if (state === 'no-passphrase')
    return <p className="text-small text-ink-muted">{S.passphrase.none}</p>;

  const remembered = state === 'remembered';
  const agentOff = state === 'agent-off';
  return (
    <div className="flex flex-col gap-1.5" aria-live="polite" aria-busy={busy || undefined}>
      <p
        className={cx(
          'flex items-start gap-1 text-small',
          remembered ? 'text-aqua-ink' : 'text-warning-ink',
        )}
      >
        <Ico icon={remembered ? Icons.ok : Icons.warning} className="mt-px" />
        <span>{remembered ? S.passphrase.remembered : S.passphrase.asks}</span>
      </p>
      <p className="text-small text-ink-muted">
        {remembered
          ? S.passphrase.rememberedHelp
          : agentOff
            ? S.passphrase.agentOffHelp
            : S.passphrase.asksHelp}
      </p>
      {remembered ? null : (
        <div className="flex flex-col items-start gap-1">
          <Button icon={Icons.key} loading={busy} onClick={() => void run(agentOff)}>
            {busy ? S.passphrase.working : agentOff ? S.passphrase.turnOn : S.passphrase.remember}
          </Button>
          <p className="text-small text-ink-muted">{S.passphrase.consoleNote}</p>
        </div>
      )}
    </div>
  );
}
