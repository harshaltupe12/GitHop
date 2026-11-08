import { useCallback, useEffect, useId, useState, type FormEvent } from 'react';
import { cx } from '../lib/cx';
import * as ipc from '../lib/ipc';
import type { IpcError, PassphraseState, RememberOutcome } from '../lib/types';
import { S } from '../strings';
import { Button } from './Button';
import { PasswordInput } from './Field';
import { Ico, Icons } from './icons';
import { useToast } from './Toast';

/**
 * Key panel line (ADR 0006, 0007): does the Windows ssh-agent remember this
 * key's passphrase? Status is icon + text (Design §11). The passphrase is typed
 * right here; the backend unlocks the key in memory, hands it to the agent and
 * never stores the passphrase. With the agent off, the same button asks Windows
 * for admin rights once (UAC) first. A console window remains as a fallback
 * for key types the app can't unlock. VS Code's Git has no terminal to ask for
 * a passphrase, so this is what makes its pushes work.
 */
export function PassphraseStatus({ profileId, keyPath }: { profileId: string; keyPath: string }) {
  const toast = useToast();
  const fieldId = useId();
  const [state, setState] = useState<PassphraseState | null>(null);
  const [busy, setBusy] = useState(false);
  const [passphrase, setPassphrase] = useState('');
  const [error, setError] = useState<string | null>(null);

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

  const run = async (turnOn: boolean, typed?: string) => {
    setBusy(true);
    setError(null);
    try {
      const out: RememberOutcome = turnOn
        ? await ipc.turnOnAgent(profileId, typed)
        : await ipc.rememberPassphrase(profileId, typed);
      if (out === 'remembered') {
        setState('remembered');
        setPassphrase('');
        toast({ message: S.passphrase.done });
      } else if (out === 'no-passphrase') {
        setState('no-passphrase');
      } else {
        const message =
          out === 'cancelled'
            ? S.passphrase.cancelled
            : out === 'agent-off'
              ? S.passphrase.stillOff
              : typed
                ? S.passphrase.notAddedInApp
                : S.passphrase.notAdded;
        toast({ kind: 'warning', message });
        load();
      }
    } catch (e) {
      // Wrong passphrase or a key type the app can't unlock: shown at the field.
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  if (!state || state === 'no-key' || state === 'unknown') return null;

  if (state === 'no-passphrase')
    return <p className="text-small text-ink-muted">{S.passphrase.none}</p>;

  const remembered = state === 'remembered';
  const agentOff = state === 'agent-off';
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (passphrase) void run(agentOff, passphrase);
  };
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
        // Not a <form>: this sits inside the profile form.
        <div className="flex flex-col gap-1.5">
          <label id={`${fieldId}-label`} htmlFor={fieldId} className="text-small text-ink">
            {S.passphrase.fieldLabel}
          </label>
          <div className="flex flex-wrap items-start gap-2">
            <div className="min-w-[12rem] flex-1">
              <PasswordInput
                id={fieldId}
                value={passphrase}
                autoComplete="off"
                aria-invalid={error ? true : undefined}
                aria-describedby={error ? `${fieldId}-error` : `${fieldId}-help`}
                onChange={(e) => {
                  setPassphrase(e.target.value);
                  setError(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') submit(e);
                }}
              />
            </div>
            <Button
              icon={Icons.key}
              loading={busy}
              disabled={!passphrase}
              onClick={() => void run(agentOff, passphrase)}
            >
              {busy
                ? S.passphrase.workingInApp
                : agentOff
                  ? S.passphrase.turnOn
                  : S.passphrase.remember}
            </Button>
          </div>
          {error ? (
            <p
              id={`${fieldId}-error`}
              role="alert"
              className="flex items-start gap-1 text-small text-blocked-text"
            >
              <Ico icon={Icons.warning} className="mt-px" />
              <span>{error}</span>
            </p>
          ) : (
            <p id={`${fieldId}-help`} className="text-small text-ink-muted">
              {S.passphrase.inAppNote}
            </p>
          )}
          <button
            type="button"
            className="self-start text-small text-aqua-text underline-offset-2 hover:underline"
            disabled={busy}
            onClick={() => void run(agentOff)}
          >
            {S.passphrase.useConsole}
          </button>
        </div>
      )}
    </div>
  );
}
