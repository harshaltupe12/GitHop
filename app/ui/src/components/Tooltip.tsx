import {
  cloneElement,
  isValidElement,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { cx } from '../lib/cx';

/**
 * Design §7.15: bg `text`, white `small` text, radius-sm, padding 4 8, 500 ms
 * delay, max width 280. Shows on hover and keyboard focus; Esc hides it.
 * `describe` adds aria-describedby (off when the tooltip repeats an aria-label).
 */
export function Tooltip({
  content,
  children,
  describe = false,
  placement = 'top',
  className,
}: {
  content: ReactNode;
  children: ReactNode;
  describe?: boolean;
  placement?: 'top' | 'bottom';
  className?: string;
}) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  const show = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOpen(true), 500);
  };
  const hide = () => {
    window.clearTimeout(timer.current);
    setOpen(false);
  };

  useEffect(() => () => window.clearTimeout(timer.current), []);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  return (
    // Hover/focus listeners only observe the wrapped control; the wrapper itself is not interactive.
    // eslint-disable-next-line jsx-a11y/no-static-element-interactions
    <span
      className={cx('relative inline-flex', className)}
      onMouseEnter={show}
      onMouseLeave={hide}
      onFocus={show}
      onBlur={hide}
    >
      {describe && isValidElement<{ 'aria-describedby'?: string }>(children)
        ? cloneElement(children, { 'aria-describedby': id })
        : children}
      <span
        id={id}
        role="tooltip"
        hidden={!open}
        className={cx(
          'gh-tooltip pointer-events-none absolute left-1/2 z-50 w-max max-w-tooltip -translate-x-1/2 rounded-sm bg-ink px-2 py-1 text-small text-white shadow-float',
          placement === 'top' ? 'bottom-full mb-1' : 'top-full mt-1',
        )}
      >
        {content}
      </span>
    </span>
  );
}
