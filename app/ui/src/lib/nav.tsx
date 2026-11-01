import { createContext, useContext } from 'react';

export type Screen = 'profiles' | 'repos' | 'fix' | 'settings';

/** One-shot requests a screen consumes on arrival (e.g. Ctrl+N, Ctrl+F, "Fix" from Repos). */
export type Intent =
  | { kind: 'new-profile' }
  | { kind: 'edit-profile'; id: string; focus?: 'owners' }
  | { kind: 'scan' }
  | { kind: 'fix-repo'; repo: string }
  | null;

export type Nav = {
  screen: Screen;
  intent: Intent;
  go: (screen: Screen, intent?: Intent) => void;
  clearIntent: () => void;
};

export const NavContext = createContext<Nav>({
  screen: 'profiles',
  intent: null,
  go: () => undefined,
  clearIntent: () => undefined,
});

export const useNav = () => useContext(NavContext);
