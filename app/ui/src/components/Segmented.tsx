import { useRef, type KeyboardEvent } from 'react';
import { cx } from '../lib/cx';
import { SlidingIndicator, useSlidingIndicator } from '../lib/motion';

/** Design §7.11 — 1 px border-input container, 28 px segments, selected = aqua. Arrow keys switch. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  labelledBy,
  disabled,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label?: string;
  labelledBy?: string;
  disabled?: boolean;
}) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const onKey = (e: KeyboardEvent, i: number) => {
    let n = -1;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') n = (i + 1) % options.length;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') n = (i - 1 + options.length) % options.length;
    if (n >= 0) {
      e.preventDefault();
      const o = options[n];
      if (o) {
        onChange(o.value);
        refs.current[n]?.focus();
      }
    }
  };
  const selectedIndex = Math.max(
    0,
    options.findIndex((o) => o.value === value),
  );
  const root = useRef<HTMLDivElement>(null);
  const thumb = useSlidingIndicator(root, '[aria-checked="true"]', value);
  return (
    <div
      ref={root}
      role="radiogroup"
      aria-label={label}
      aria-labelledby={labelledBy}
      data-indicator={thumb.box ? 'ready' : undefined}
      className="gh-seg relative inline-flex rounded-md border border-line-input bg-surface p-0.5"
    >
      {/* Aqua thumb slides to the selected segment (MOTION.md). */}
      <SlidingIndicator {...thumb} className="rounded-sm bg-aqua" />
      {options.map((o, i) => {
        const sel = i === selectedIndex && o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={sel}
            tabIndex={i === selectedIndex ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKey(e, i)}
            className={cx(
              'transition-ui min-h-7 rounded-sm px-3 text-body',
              sel
                ? 'bg-aqua font-medium text-aqua-ink'
                : 'text-ink hover:bg-surface-hover active:bg-surface-pressed',
              'disabled:cursor-not-allowed disabled:text-ink-disabled',
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
