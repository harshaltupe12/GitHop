import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Avatar } from '../components/Avatar';
import { Banner } from '../components/Banner';
import { Button } from '../components/Button';
import { Icons } from '../components/icons';
import { useToast } from '../components/Toast';
import { applyProfiles, STATE_KEY } from '../lib/appState';
import { cx } from '../lib/cx';
import { guessLabel, slugify, sortProfiles } from '../lib/format';
import * as ipc from '../lib/ipc';
import { invalidate } from '../lib/query';
import { KEYS, readJson, removeKey, writeJson } from '../lib/storage';
import type { AppState, IpcError, Profile } from '../lib/types';
import { S } from '../strings';
import { ProfileForm } from './ProfileForm';

const TOTAL = 6;

function initialStep(state: AppState): number {
  const stored = readJson<number | null>(KEYS.onboardingStep, null);
  if (typeof stored === 'number' && stored >= 0 && stored < TOTAL) {
    // Never resume on a step whose prerequisite is gone.
    if (stored >= 3 && !state.onboarding.has_profiles) return 2;
    return stored;
  }
  if (!state.onboarding.has_profiles) return 0;
  return state.onboarding.hooks_installed ? 5 : 4;
}

/** Design §8.6 — full-window first-run steps, progress dots, Back/Next. Resumable. */
export function Onboarding({ state, onFinish }: { state: AppState; onFinish: () => void }) {
  const toast = useToast();
  const [step, setStep] = useState(() => initialStep(state));
  const [adding, setAdding] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const headRef = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    writeJson(KEYS.onboardingStep, step);
    setError(null);
    headRef.current?.focus();
  }, [step]);

  const go = (n: number) => setStep(Math.max(0, Math.min(TOTAL - 1, n)));
  const g = state.onboarding.global_identity ?? null;
  const guess = guessLabel(g?.email);
  const firstLabel = guess === 'personal' ? S.onboarding.guessPersonal : S.onboarding.guessOffice;
  const secondLabel = guess === 'personal' ? S.onboarding.guessOffice : S.onboarding.guessPersonal;
  const list = sortProfiles(state.profiles);

  const afterFirstSave = async (saved: Profile, hadDefault: boolean) => {
    if (!hadDefault) {
      try {
        // Backs up the global git config first (IPC.md set_default).
        applyProfiles(await ipc.setDefault(saved.id));
        toast({ message: S.onboarding.safety.backupMade });
      } catch (e) {
        toast({ kind: 'error', message: (e as IpcError).message });
      }
    }
    go(3);
  };

  const install = async () => {
    setInstalling(true);
    setError(null);
    try {
      await ipc.installHooks();
      invalidate(STATE_KEY);
      go(5);
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setInstalling(false);
    }
  };

  const finish = () => {
    writeJson(KEYS.onboardingDone, true);
    removeKey(KEYS.onboardingStep);
    onFinish();
  };

  let title = '';
  let content: ReactNode = null;
  let next: ReactNode = null;

  switch (step) {
    case 0:
      title = S.onboarding.welcome.title;
      content = (
        <div className="flex flex-col gap-4">
          <p className="text-body text-ink">{S.onboarding.welcome.body}</p>
          <Banner kind="privacy" live={false}>
            {S.onboarding.welcome.privacy}
          </Banner>
        </div>
      );
      next = (
        <Button variant="primary" onClick={() => go(1)}>
          {S.common.next}
        </Button>
      );
      break;
    case 1:
      title = S.onboarding.safety.title;
      content = (
        <div className="flex flex-col gap-4">
          <p>{S.onboarding.safety.body}</p>
          <section
            aria-labelledby="ob-current"
            className="rounded-lg border border-line bg-surface px-4 py-3"
          >
            <h2 id="ob-current" className="text-body font-medium">
              {S.onboarding.safety.current}
            </h2>
            {g ? (
              <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
                <dt className="text-small text-ink-muted">{S.onboarding.safety.nameLabel}</dt>
                <dd>{g.name}</dd>
                <dt className="text-small text-ink-muted">{S.onboarding.safety.emailLabel}</dt>
                <dd className="truncate font-mono text-mono">{g.email}</dd>
              </dl>
            ) : (
              <p className="mt-1 text-ink-muted">{S.onboarding.safety.none}</p>
            )}
          </section>
          <Banner kind="info" icon={Icons.shield} live={false}>
            {S.onboarding.safety.backupLocation}
          </Banner>
        </div>
      );
      next = (
        <Button variant="primary" onClick={() => go(2)}>
          {S.common.next}
        </Button>
      );
      break;
    case 2:
      title = S.onboarding.first.title;
      if (list.length) {
        content = <ProfileList profiles={list} defaultId={state.profiles.default} />;
        next = (
          <Button variant="primary" onClick={() => go(3)}>
            {S.common.next}
          </Button>
        );
      } else {
        content = (
          <div className="flex flex-col gap-4">
            <p>{g ? S.onboarding.first.guess(firstLabel) : S.onboarding.first.fresh}</p>
            <ProfileForm
              embedded
              heading={firstLabel}
              profiles={state.profiles}
              seed={
                g
                  ? { label: firstLabel, id: slugify(firstLabel), name: g.name, email: g.email }
                  : undefined
              }
              submitLabel={S.common.next}
              onSaved={(set, saved) => {
                const hadDefault = !!state.profiles.default;
                applyProfiles(set);
                void afterFirstSave(saved, hadDefault);
              }}
            />
          </div>
        );
      }
      break;
    case 3:
      title = S.onboarding.another.title;
      content = adding ? (
        <ProfileForm
          embedded
          heading={secondLabel}
          profiles={state.profiles}
          seed={{ label: secondLabel, id: slugify(secondLabel) }}
          onCancel={() => setAdding(false)}
          onSaved={(set, saved) => {
            applyProfiles(set);
            setAdding(false);
            toast({ message: S.onboarding.another.added(saved.label) });
          }}
        />
      ) : (
        <div className="flex flex-col gap-4">
          <p>{S.onboarding.another.body}</p>
          <ProfileList profiles={list} defaultId={state.profiles.default} />
          <div>
            <Button icon={Icons.add} onClick={() => setAdding(true)}>
              {S.onboarding.another.add}
            </Button>
          </div>
        </div>
      );
      next = adding ? null : (
        <>
          <Button variant="ghost" onClick={() => go(4)}>
            {S.common.skip}
          </Button>
          <Button variant="primary" onClick={() => go(4)}>
            {S.common.next}
          </Button>
        </>
      );
      break;
    case 4: {
      title = S.onboarding.protect.title;
      const h = state.hooks;
      const existing = h.global_hooks_path && !h.points_to_us ? h.global_hooks_path : '';
      content = (
        <div className="flex flex-col gap-4">
          <p>{S.onboarding.protect.body}</p>
          {existing ? (
            <Banner kind="info" live={false}>
              {S.onboarding.protect.existing(existing)}
            </Banner>
          ) : null}
          {state.onboarding.hooks_installed ? (
            <Banner kind="success">{S.onboarding.protect.on}</Banner>
          ) : null}
          {error ? <Banner kind="blocked">{error}</Banner> : null}
        </div>
      );
      next = state.onboarding.hooks_installed ? (
        <Button variant="primary" onClick={() => go(5)}>
          {S.common.next}
        </Button>
      ) : (
        <Button variant="primary" icon={Icons.shield} onClick={install} loading={installing}>
          {installing ? S.onboarding.protect.installing : S.onboarding.protect.action}
        </Button>
      );
      break;
    }
    default:
      title = S.onboarding.done.title;
      content = <p>{S.onboarding.done.body}</p>;
      next = (
        <Button variant="primary" onClick={finish}>
          {S.onboarding.done.finish}
        </Button>
      );
  }

  return (
    <main className="fade-in flex h-full flex-col overflow-y-auto bg-bg" aria-labelledby="ob-title">
      <div className="mx-auto flex w-full max-w-[40rem] flex-1 flex-col px-6 py-8">
        <ol
          className="mb-8 flex items-center justify-center gap-2"
          aria-label={S.onboarding.progress(step + 1, TOTAL)}
        >
          {Array.from({ length: TOTAL }, (_, i) => (
            <li
              key={i}
              aria-current={i === step ? 'step' : undefined}
              className={cx(
                'transition-colour-base h-2 rounded-full',
                i === step ? 'w-6 bg-aqua-strong' : i < step ? 'w-2 bg-aqua' : 'w-2 bg-line-input',
              )}
            >
              <span className="sr-only">{S.onboarding.progress(i + 1, TOTAL)}</span>
            </li>
          ))}
        </ol>
        <h1 id="ob-title" ref={headRef} tabIndex={-1} className="mb-4 text-display text-ink">
          {title}
        </h1>
        <div className="flex-1">{content}</div>
        <div className="mt-8 flex flex-wrap items-center justify-between gap-2 border-t border-line pt-4">
          <div>
            {step > 0 && step < TOTAL - 1 ? (
              <Button onClick={() => go(step - 1)}>{S.common.back}</Button>
            ) : null}
          </div>
          <div className="flex flex-wrap gap-2">{next}</div>
        </div>
      </div>
    </main>
  );
}

function ProfileList({ profiles, defaultId }: { profiles: Profile[]; defaultId?: string | null }) {
  return (
    <ul className="flex flex-col gap-2">
      {profiles.map((p) => (
        <li
          key={p.id}
          className={cx(
            'flex items-center gap-3 rounded-lg bg-surface px-4 py-3',
            p.id === defaultId ? 'border-[1.5px] border-aqua' : 'border border-line',
          )}
        >
          <Avatar label={p.label} color={p.color} size={30} />
          <span className="min-w-0 flex-1">
            <span className="block text-heading">{p.label}</span>
            <span className="block truncate font-mono text-mono text-ink-muted">{p.email}</span>
          </span>
        </li>
      ))}
    </ul>
  );
}
