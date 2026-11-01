import { render } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement } from 'react';
import { ToastProvider } from '../components/Toast';
import { NavContext, type Nav } from '../lib/nav';
import { vi } from 'vitest';

export function renderWith(ui: ReactElement, nav: Partial<Nav> = {}) {
  const value: Nav = {
    screen: 'profiles',
    intent: null,
    go: vi.fn(),
    clearIntent: vi.fn(),
    ...nav,
  };
  const user = userEvent.setup();
  const r = render(
    <ToastProvider>
      <NavContext.Provider value={value}>{ui}</NavContext.Provider>
    </ToastProvider>,
  );
  return { ...r, user, nav: value };
}
