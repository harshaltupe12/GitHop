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
import { motionEnabled, tokenMs, useFlip, useSwipeDismiss } from '../lib/motion';
import { S } from '../strings';
import { Ico, Icons, type Icon } from './icons';

export type ToastKind = 'success' | 'info' | 'warning' | 'error';
export type ToastInput = {
  kind?: ToastKind;
  message: string;
  action?: { label: string; onClick: () => void };
};
/** `leaving`: playing its exit animation (MOTION.md); removed on animationend. */
type ToastItem = ToastInput & { id: number; kind: ToastKind; leaving?: boolean };

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

  const stack = useRef<HTMLElement>(null);
  useFlip(stack, items, slots);

  const remove = useCallback((id: number) => {
    setItems((xs) => xs.filter((x) => x.id !== id));
  }, []);

  const dismiss = useCallback(
    (id: number) => {
      const t = timers.current.get(id);
      if (t) window.clearTimeout(t);
      timers.current.delete(id);
      if (!motionEnabled()) return remove(id);
      setItems((xs) => xs.map((x) => (x.id === id ? { ...x, leaving: true } : x)));
    },
    [remove],
  );

  const push = useCallback(
    (t: ToastInput) => {
      const id = ++seq.current;
      const kind = t.kind ?? 'success';
      setItems((xs) => {
        // At most three on screen; the oldest steps out (animated when motion is on).
        const live = xs.filter((x) => !x.leaving);
        const drop = new Set(live.slice(0, Math.max(0, live.length - 2)).map((x) => x.id));
        const kept = motionEnabled()
          ? xs.map((x) => (drop.has(x.id) ? { ...x, leaving: true } : x))
          : xs.filter((x) => !drop.has(x.id));
        return [...kept, { ...t, kind, id }];
      });
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
    <ToastShell
      key={t.id}
      id={t.id}
      kind={t.kind}
      leaving={!!t.leaving}
      dismiss={dismiss}
      remove={remove}
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
    </ToastShell>
  ));

  // Same motion in both homes (corner and dialog slot); `stack` follows the live one.
  return (
    <SlotCtx.Provider value={register}>
      <Ctx.Provider value={value}>
        {children}
        {slot ? (
          createPortal(
            <section
              aria-label={S.common.notifications}
              aria-live="polite"
              ref={stack}
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
            ref={stack}
            className="pointer-events-none fixed bottom-4 right-4 z-[60] flex w-[min(24rem,calc(100vw-2rem))] flex-col items-end gap-2"
          >
            {list}
          </section>
        )}
      </Ctx.Provider>
    </SlotCtx.Provider>
  );
}

/**
 * One toast's surface + motion (MOTION.md): rises in, fades out while `leaving`
 * (then `remove`), slides away when dragged right. Visuals per Design §7.16.
 */
function ToastShell({
  id,
  kind,
  leaving,
  dismiss,
  remove,
  children,
}: {
  id: number;
  kind: ToastKind;
  leaving: boolean;
  dismiss: (id: number) => void;
  remove: (id: number) => void;
  children: ReactNode;
}) {
  const swipe = useSwipeDismiss(() => dismiss(id));
  useEffect(() => {
    if (!leaving) return;
    // Safety net if animationend never arrives (e.g. the window is hidden).
    const t = window.setTimeout(() => remove(id), tokenMs('--dur-exit', 120) + 80);
    return () => window.clearTimeout(t);
  }, [leaving, id, remove]);
  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      data-flip={id}
      data-state={leaving ? 'closed' : 'open'}
      onAnimationEnd={(e) => {
        if (leaving && e.target === e.currentTarget) remove(id);
      }}
      {...swipe}
      className="gh-toast pointer-events-auto flex w-full items-start gap-2 rounded-md bg-surface px-3 py-2.5 text-body text-ink shadow-float"
    >
      {children}
    </div>
  );
}
