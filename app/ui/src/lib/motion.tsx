// Motion helpers (see app/ui/MOTION.md). CSS does the animating; these hooks only
// decide *when* (keep a closing layer mounted, mark rows that are new, measure an
// indicator's target). No animation library, no rAF loops, no per-frame layout reads.
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type AnimationEvent,
  type PointerEvent,
  type RefObject,
} from 'react';
import { cx } from './cx';

/**
 * True when the platform can animate and the user hasn't asked for less motion
 * (Windows "Animation effects" off maps to `prefers-reduced-motion: reduce`).
 * jsdom has no matchMedia, so unit tests get the instant path unless they stub it.
 */
export function motionEnabled(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  return !window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** A motion token from tokens.css, in ms (e.g. `--dur-exit`). */
export function tokenMs(name: string, fallback: number): number {
  if (typeof document === 'undefined') return fallback;
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const n = parseFloat(raw);
  if (!Number.isFinite(n)) return fallback;
  return raw.endsWith('ms') ? n : raw.endsWith('s') ? n * 1000 : n;
}

function tokenValue(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/** Exit animations: `--dur-exit` (120 ms). A safety timer adds this much on top. */
const EXIT_GRACE_MS = 80;

export type PresenceState = 'open' | 'closed';

/**
 * Keeps a layer (dialog, menu) mounted while its exit animation plays.
 * Render it while `mounted`; put `data-state={state}` on the animated root and
 * `onAnimationEnd` on the same element. Unmounts on that element's own
 * animationend, or after a safety timeout. Without motion it unmounts at once.
 */
export function usePresence(open: boolean) {
  const [s, setS] = useState({ open, exiting: false });
  let exiting = s.exiting;
  if (s.open !== open) {
    // Derived during render so the closing frame is never skipped.
    exiting = !open && motionEnabled();
    setS({ open, exiting });
  }

  const finish = useCallback(() => setS((x) => (x.exiting ? { ...x, exiting: false } : x)), []);

  useEffect(() => {
    if (!exiting) return;
    const t = window.setTimeout(finish, tokenMs('--dur-exit', 120) + EXIT_GRACE_MS);
    return () => window.clearTimeout(t);
  }, [exiting, finish]);

  const onAnimationEnd = useCallback(
    (e: AnimationEvent<HTMLElement>) => {
      if (e.target === e.currentTarget) finish();
    },
    [finish],
  );

  return {
    mounted: open || exiting,
    exiting,
    state: (open ? 'open' : 'closed') as PresenceState,
    onAnimationEnd,
  };
}

/**
 * Counts changes of `value` after the first render. Use it as a `key` to replay
 * a short cross-fade when a status changes — never on first mount.
 */
export function useChangeCount(value: unknown): number {
  const [s, setS] = useState({ value, n: 0 });
  if (!Object.is(s.value, value)) {
    const next = { value, n: s.n + 1 };
    setS(next);
    return next.n;
  }
  return s.n;
}

/**
 * Which keys arrived after the list first rendered. Items present on mount never
 * animate (a big table must not fade in row by row); later arrivals do. When
 * more than `cap` arrive at once they don't animate either (a fresh rescan).
 */
export function useArrivals(keys: readonly string[], cap = 24): (key: string) => boolean {
  const known = useRef<Set<string> | null>(null);
  const arrived = useRef(new Set<string>());
  if (known.current === null) known.current = new Set(keys);
  const fresh = keys.filter((k) => !known.current?.has(k));
  if (fresh.length) {
    const animate = fresh.length <= cap;
    for (const k of fresh) {
      known.current.add(k);
      if (animate) arrived.current.add(k);
    }
  }
  return (key: string) => arrived.current.has(key);
}

export type Box = { x: number; y: number; w: number; h: number };
const sameBox = (a: Box | null, b: Box | null) =>
  a === b || (!!a && !!b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h);

/**
 * Measures the active child (`selector`) of `container` with offset* (layout
 * values, unaffected by running transforms) so a single indicator element can
 * glide to it. Re-measures when the container resizes (text size, fonts).
 * `animate` turns on one frame after the first measurement, so the first
 * placement never animates.
 */
export function useSlidingIndicator(
  container: RefObject<HTMLElement | null>,
  selector: string,
  active: unknown,
) {
  const [box, setBox] = useState<Box | null>(null);
  const [animate, setAnimate] = useState(false);

  useLayoutEffect(() => {
    const root = container.current;
    if (!root) return;
    const measure = () => {
      const el = root.querySelector<HTMLElement>(selector);
      const next =
        el && el.offsetWidth > 0
          ? { x: el.offsetLeft, y: el.offsetTop, w: el.offsetWidth, h: el.offsetHeight }
          : null;
      setBox((prev) => (sameBox(prev, next) ? prev : next));
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(measure);
    ro.observe(root);
    return () => ro.disconnect();
  }, [container, selector, active]);

  useEffect(() => {
    if (!box || animate) return;
    const id = window.requestAnimationFrame(() => setAnimate(true));
    return () => window.cancelAnimationFrame(id);
  }, [box, animate]);

  return { box, animate };
}

/** The gliding highlight drawn behind the active item. Decorative. */
export function SlidingIndicator({
  box,
  animate,
  className,
}: {
  box: Box | null;
  animate: boolean;
  className?: string;
}) {
  if (!box) return null;
  return (
    <span
      aria-hidden="true"
      data-animate={animate || undefined}
      className={cx('gh-indicator pointer-events-none absolute left-0 top-0', className)}
      style={{
        width: box.w,
        height: box.h,
        transform: `translate(${box.x}px, ${box.y}px)`,
      }}
    />
  );
}

/**
 * FLIP for a small stack (toasts): after each commit, children marked
 * `data-flip="<key>"` that moved are animated from their old spot with WAAPI
 * (`translate`, composited, additive so a second change mid-flight doesn't jump).
 * One layout read per child per change — never per frame.
 */
export function useFlip(container: RefObject<HTMLElement | null>, dep: unknown, home?: unknown) {
  const last = useRef(new Map<string, number>());
  const lastRoot = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    const root = container.current;
    if (!root) return;
    // Moved to another container (corner ↔ dialog slot): re-baseline, don't fly across.
    if (root !== lastRoot.current) {
      lastRoot.current = root;
      last.current = new Map();
    }
    const originTop = root.getBoundingClientRect().top;
    const els = Array.from(root.querySelectorAll<HTMLElement>(':scope > [data-flip]'));
    const next = new Map<string, number>();
    const go = motionEnabled();
    const duration = go ? tokenMs('--dur-base', 160) : 0;
    const easing = go ? tokenValue('--ease', 'ease-out') : 'linear';
    for (const el of els) {
      const key = el.dataset.flip ?? '';
      const top = originTop + el.offsetTop;
      next.set(key, top);
      const prev = last.current.get(key);
      if (
        go &&
        prev !== undefined &&
        Math.abs(prev - top) > 0.5 &&
        typeof el.animate === 'function'
      ) {
        el.animate([{ translate: `0 ${prev - top}px` }, { translate: '0 0' }], {
          duration,
          easing,
          composite: 'add',
        });
      }
    }
    last.current = next;
  }, [container, dep, home]);
}

/** Swipe distance (px) or speed (px/ms) that dismisses a toast. */
const SWIPE_DISTANCE = 56;
const SWIPE_VELOCITY = 0.11;

/**
 * Drag a toast towards the edge it lives on (right) to dismiss it; a quick flick
 * is enough. Dragging the other way is damped. Buttons inside keep working.
 */
export function useSwipeDismiss(onDismiss: () => void) {
  const start = useRef<{ x: number; t: number; id: number } | null>(null);

  const onPointerDown = (e: PointerEvent<HTMLElement>) => {
    if (e.button !== 0 || start.current) return;
    if ((e.target as HTMLElement).closest('button, a, input')) return;
    start.current = { x: e.clientX, t: e.timeStamp, id: e.pointerId };
    e.currentTarget.setPointerCapture?.(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent<HTMLElement>) => {
    const s = start.current;
    if (!s || e.pointerId !== s.id) return;
    const dx = e.clientX - s.x;
    const el = e.currentTarget;
    el.dataset.swiping = '';
    el.style.translate = `${dx > 0 ? dx : dx / 5}px 0`;
  };

  const end = (e: PointerEvent<HTMLElement>) => {
    const s = start.current;
    if (!s || e.pointerId !== s.id) return;
    start.current = null;
    const el = e.currentTarget;
    el.releasePointerCapture?.(e.pointerId);
    delete el.dataset.swiping;
    const dx = e.clientX - s.x;
    const velocity = dx / Math.max(1, e.timeStamp - s.t);
    if (e.type === 'pointerup' && (dx >= SWIPE_DISTANCE || (dx > 8 && velocity > SWIPE_VELOCITY))) {
      el.dataset.swiped = '';
      onDismiss();
    } else {
      el.style.translate = '';
    }
  };

  return { onPointerDown, onPointerMove, onPointerUp: end, onPointerCancel: end };
}

/**
 * Tooltip skip-delay group: once one tooltip is open (or closed under
 * `SKIP_MS` ago), the next opens at once and without animation.
 */
const SKIP_MS = 300;
const tips = { open: 0, closedAt: Number.NEGATIVE_INFINITY };
export const tooltipGroup = {
  skipDelay(): boolean {
    if (tips.open > 0) return true;
    const d = Date.now() - tips.closedAt;
    return d >= 0 && d < SKIP_MS;
  },
  opened(): void {
    tips.open += 1;
  },
  closed(): void {
    tips.open = Math.max(0, tips.open - 1);
    tips.closedAt = Date.now();
  },
  /** Tests only. */
  reset(): void {
    tips.open = 0;
    tips.closedAt = Number.NEGATIVE_INFINITY;
  },
};
