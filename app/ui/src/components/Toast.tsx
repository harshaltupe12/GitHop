import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { cx } from '../lib/cx';
import { S } from '../strings';
import { Ico, Icons, type Icon } from './icons';

export type ToastKind = 'success' | 'info' | 'warning' | 'error';
export type ToastInput = {
  kind?: ToastKind;
  message: string;
  action?: { label: string; onClick: () => void };
};
type ToastItem = ToastInput & { id: number; kind: ToastKind };

const ICON: Record<ToastKind, { icon: Icon; cls: string }> = {
  success: { icon: Icons.ok, cls: 'text-aqua-text' },
  info: { icon: Icons.info, cls: 'text-ink-muted' },
  warning: { icon: Icons.warning, cls: 'text-warning-ink' },
  error: { icon: Icons.blocked, cls: 'text-blocked-text' },
};

const Ctx = createContext<(t: ToastInput) => void>(() => undefined);

export const useToast = () => useContext(Ctx);

/**
 * An open dialog hands the toast list a place in its own layer, directly below
 * the dialog panel, so a toast never covers the dialog's buttons and isn't
 * hidden behind the scrim either (Design §7.13 / §7.16).
 */
type SlotApi = (key: string, el: HTMLElement | null) => void;
const SlotCtx = createContext<SlotApi>(() => undefined);

/** Used by Dialog: returns a ref callback for its toast slot element. */
export function useToastSlot(key: string): (el: HTMLElement | null) => void {
  const register = useContext(SlotCtx);
  return useCallback((el: HTMLElement | null) => register(key, el), [register, key]);
}

/**
 * Design §7.16 — bottom-right, surface, radius-md, shadow, icon + one line +
 * optional action ("Undo"). Auto-dismiss after 4 s, except errors.
 */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const [slots, setSlots] = useState<Array<{ key: string; el: HTMLElement }>>([]);
  const seq = useRef(0);
  const timers = useRef(new Map<number, number>());

  const dismiss = useCallback((id: number) => {
    setItems((xs) => xs.filter((x) => x.id !== id));
    const t = timers.current.get(id);
    if (t) window.clearTimeout(t);
    timers.current.delete(id);
  }, []);

  const push = useCallback(
    (t: ToastInput) => {
      const id = ++seq.current;
      const kind = t.kind ?? 'success';
      setItems((xs) => [...xs.slice(-2), { ...t, kind, id }]);
      if (kind !== 'error') {
        timers.current.set(
          id,
          window.setTimeout(() => dismiss(id), 4000),
        );
      }
    },
    [dismiss],
  );

  const register = useCallback<SlotApi>((key, el) => {
    setSlots((xs) => {
      const rest = xs.filter((x) => x.key !== key);
      return el ? [...rest, { key, el }] : rest;
    });
  }, []);

  useEffect(() => {
    const ts = timers.current;
    return () => ts.forEach((t) => window.clearTimeout(t));
  }, []);

  const value = useMemo(() => push, [push]);
  // The newest open dialog wins (dialogs don't normally stack).
  const slot = slots[slots.length - 1]?.el ?? null;

  const list = items.map((t) => (
    <div
      key={t.id}
      role={t.kind === 'error' ? 'alert' : 'status'}
      className="gh-toast menu-in pointer-events-auto flex w-full items-start gap-2 rounded-md bg-surface px-3 py-2.5 text-body text-ink shadow-float"
    >
      <Ico icon={ICON[t.kind].icon} className={cx('mt-0.5', ICON[t.kind].cls)} />
      <span className="min-w-0 flex-1">{t.message}</span>
      {t.action ? (
        <button
          type="button"
          className="shrink-0 rounded-sm font-medium text-aqua-text hover:underline"
          onClick={() => {
            dismiss(t.id);
            t.action?.onClick();
          }}
        >
          {t.action.label}
        </button>
      ) : null}
      <button
        type="button"
        aria-label={S.common.dismiss}
        onClick={() => dismiss(t.id)}
        className="transition-ui -mr-1 inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-ink-muted hover:bg-surface-hover"
      >
        <Ico icon={Icons.dismiss} />
      </button>
    </div>
  ));

  return (
    <SlotCtx.Provider value={register}>
      <Ctx.Provider value={value}>
        {children}
        {slot ? (
          createPortal(
            <section
              aria-label={S.common.notifications}
              aria-live="polite"
              className="pointer-events-none flex w-full flex-col items-end gap-2"
            >
              {list}
            </section>,
            slot,
          )
        ) : (
          <section
            aria-label={S.common.notifications}
            aria-live="polite"
            className="pointer-events-none fixed bottom-4 right-4 z-[60] flex w-[min(24rem,calc(100vw-2rem))] flex-col items-end gap-2"
          >
            {list}
          </section>
        )}
      </Ctx.Provider>
    </SlotCtx.Provider>
  );
}
