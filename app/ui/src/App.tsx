import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { Banner } from './components/Banner';
import { Button } from './components/Button';
import { DefaultPanel } from './components/DefaultPanel';
import { Icons } from './components/icons';
import { NavItem } from './components/NavItem';
import { Spinner } from './components/Spinner';
import { ToastProvider, useToast } from './components/Toast';
import { applyProfiles, hooksHealthy, STATE_KEY, useAppState } from './lib/appState';
import { profileById } from './lib/format';
import * as ipc from './lib/ipc';
import { NavContext, useNav as useContextNav, type Intent, type Nav, type Screen } from './lib/nav';
import { SlidingIndicator, useSlidingIndicator } from './lib/motion';
import { invalidate } from './lib/query';
import { KEYS, readJson } from './lib/storage';
import type { AppState, IpcError } from './lib/types';
import { FixScreen } from './screens/Fix';
import { Onboarding } from './screens/Onboarding';
import { ProfilesScreen } from './screens/Profiles';
import { ReposScreen } from './screens/Repos';
import { SettingsScreen } from './screens/Settings';
import { S } from './strings';

const SCREENS: { id: Screen; label: string; icon: keyof typeof Icons; sel: keyof typeof Icons }[] =
  [
    { id: 'profiles', label: S.nav.profiles, icon: 'profiles', sel: 'profilesSelected' },
    { id: 'repos', label: S.nav.repos, icon: 'repos', sel: 'reposSelected' },
    { id: 'fix', label: S.nav.fix, icon: 'fix', sel: 'fixSelected' },
    { id: 'settings', label: S.nav.settings, icon: 'settings', sel: 'settingsSelected' },
  ];

export function App() {
  return (
    <ToastProvider>
      <Shell />
    </ToastProvider>
  );
}

function needsOnboarding(s: AppState): boolean {
  return (
    !s.onboarding.has_profiles ||
    (!s.onboarding.hooks_installed && !readJson<boolean>(KEYS.onboardingDone, false))
  );
}

export function Shell() {
  const { data: state, error, refetch } = useAppState();
  const [screen, setScreen] = useState<Screen>('profiles');
  const [intent, setIntent] = useState<Intent>(null);
  const [onboarding, setOnboarding] = useState(false);

  // Refetch on window focus and when the backend says files changed.
  useEffect(() => {
    const onFocus = () => invalidate(STATE_KEY);
    window.addEventListener('focus', onFocus);
    const subs = [
      ipc.onProfilesChanged(() => invalidate()).catch(() => () => undefined),
      ipc.onTrayDefaultChanged((set) => applyProfiles(set)).catch(() => () => undefined),
    ];
    return () => {
      window.removeEventListener('focus', onFocus);
      subs.forEach((p) => void p.then((u) => u()));
    };
  }, []);

  useEffect(() => {
    if (state && needsOnboarding(state)) setOnboarding(true);
  }, [state]);

  const go = useCallback((s: Screen, i: Intent = null) => {
    setScreen(s);
    setIntent(i);
  }, []);
  const clearIntent = useCallback(() => setIntent(null), []);
  const nav: Nav = useMemo(
    () => ({ screen, intent, go, clearIntent }),
    [screen, intent, go, clearIntent],
  );

  // Shortcuts (Design §11): Ctrl+1..4, Ctrl+N, Ctrl+F.
  useEffect(() => {
    if (onboarding) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
      if (document.querySelector('[role="dialog"]')) return;
      const n = Number(e.key);
      if (n >= 1 && n <= SCREENS.length) {
        e.preventDefault();
        go(SCREENS[n - 1]!.id);
      } else if (e.key.toLowerCase() === 'n') {
        e.preventDefault();
        go('profiles', { kind: 'new-profile' });
      } else if (e.key.toLowerCase() === 'f') {
        e.preventDefault();
        go('repos', { kind: 'scan' });
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [go, onboarding]);

  if (!state) {
    if (error) return <ConfigError message={error.message} onRetry={() => void refetch()} full />;
    return (
      <div className="flex h-full items-center justify-center gap-2 text-ink-muted" role="status">
        <Spinner />
        {S.app.loading}
      </div>
    );
  }

  if (onboarding || needsOnboarding(state)) {
    return (
      <Onboarding
        state={state}
        onFinish={() => {
          setOnboarding(false);
          go('profiles');
        }}
      />
    );
  }

  return (
    <NavContext.Provider value={nav}>
      <Layout state={state} error={error} onRetry={() => void refetch()} />
    </NavContext.Provider>
  );
}

function Layout({
  state,
  error,
  onRetry,
}: {
  state: AppState;
  error?: IpcError;
  onRetry: () => void;
}) {
  const toast = useToast();
  const { screen, go } = useContextNav();
  const navRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const navRef = useRef<HTMLElement>(null);
  const indicator = useSlidingIndicator(navRef, '[aria-current="page"]', screen);
  const [installing, setInstalling] = useState(false);

  const onNavKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    let n = -1;
    if (e.key === 'ArrowDown') n = (i + 1) % SCREENS.length;
    if (e.key === 'ArrowUp') n = (i - 1 + SCREENS.length) % SCREENS.length;
    if (e.key === 'Home') n = 0;
    if (e.key === 'End') n = SCREENS.length - 1;
    if (n >= 0) {
      e.preventDefault();
      navRefs.current[n]?.focus();
    }
  };

  const setDefault = async (id: string) => {
    try {
      applyProfiles(await ipc.setDefault(id));
      toast({ message: S.profiles.defaultSet(profileById(state.profiles, id)?.label ?? id) });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    }
  };

  const turnOn = async () => {
    setInstalling(true);
    try {
      await ipc.installHooks();
      invalidate(STATE_KEY);
      toast({ message: S.states.hooksOn });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setInstalling(false);
    }
  };

  const current = screen;
  return (
    <div className="flex h-full min-h-0">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:z-50 focus:rounded-md focus:bg-surface focus:px-3 focus:py-2"
      >
        {S.app.skipToContent}
      </a>
      <aside className="flex w-sidebar shrink-0 flex-col justify-between border-r border-line bg-surface p-2">
        <nav
          ref={navRef}
          aria-label={S.app.mainNav}
          data-indicator={indicator.box ? 'ready' : undefined}
          className="relative"
        >
          {/* Selected background glides between items (MOTION.md). */}
          <SlidingIndicator {...indicator} className="rounded-md bg-aqua-tint" />
          <ul className="flex flex-col gap-0.5">
            {SCREENS.map((s, i) => (
              <li key={s.id}>
                <NavItem
                  ref={(el) => {
                    navRefs.current[i] = el;
                  }}
                  label={s.label}
                  icon={Icons[s.icon]}
                  selectedIcon={Icons[s.sel]}
                  selected={current === s.id}
                  onSelect={() => go(s.id)}
                  onKeyDown={(e) => onNavKey(e, i)}
                  shortcut={`Control+${i + 1}`}
                />
              </li>
            ))}
          </ul>
        </nav>
        {state.profiles.profile.length ? (
          <DefaultPanel profiles={state.profiles} onChange={(id) => void setDefault(id)} />
        ) : null}
      </aside>
      <main id="main" tabIndex={-1} className="min-w-0 flex-1 overflow-y-auto">
        <div key={current} className="fade-in mx-auto max-w-content p-6">
          {error ? (
            <div className="mb-4">
              <ConfigError message={error.message} onRetry={onRetry} />
            </div>
          ) : null}
          {!hooksHealthy(state) ? (
            <Banner
              kind="warning"
              className="mb-4"
              action={{ label: S.states.turnOn, onClick: () => void turnOn(), busy: installing }}
            >
              {S.states.hooksMissing}
            </Banner>
          ) : null}
          {current === 'profiles' ? (
            <ProfilesScreen profiles={state.profiles} />
          ) : current === 'repos' ? (
            <ReposScreen profiles={state.profiles} />
          ) : current === 'fix' ? (
            <FixScreen profiles={state.profiles} />
          ) : (
            <SettingsScreen state={state} />
          )}
        </div>
      </main>
    </div>
  );
}

/** Design §8.7 "Config read error": blocked banner + "Restore last backup". */
function ConfigError({
  message,
  onRetry,
  full = false,
}: {
  message: string;
  onRetry: () => void;
  full?: boolean;
}) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const restore = async () => {
    setBusy(true);
    try {
      applyProfiles(await ipc.restoreLastBackup());
      toast({ message: S.states.restored });
      onRetry();
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setBusy(false);
    }
  };
  const banner = (
    <Banner
      kind="blocked"
      action={{ label: S.states.restoreBackup, onClick: () => void restore(), busy }}
    >
      <span className="block font-medium">{S.states.configError}</span>
      {message ? <span className="block text-small">{message}</span> : null}
    </Banner>
  );
  if (!full) return banner;
  return (
    <main className="mx-auto flex h-full max-w-content flex-col justify-center gap-4 p-6">
      {banner}
      <div>
        <Button icon={Icons.undo} onClick={onRetry}>
          {S.common.retry}
        </Button>
      </div>
    </main>
  );
}
