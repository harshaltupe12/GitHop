import { cx } from '../lib/cx';

/**
 * Design §7.12 — 32 × 18 switch. Off: track `border` with `border-input`
 * outline; on: track aqua, white knob. Exposed as role="switch".
 */
export function Toggle({
  checked,
  onChange,
  labelledBy,
  describedBy,
  label,
  disabled,
  busy,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  labelledBy?: string;
  describedBy?: string;
  label?: string;
  disabled?: boolean;
  busy?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      aria-label={label}
      aria-busy={busy || undefined}
      aria-disabled={busy || undefined}
      disabled={disabled}
      onClick={() => !busy && onChange(!checked)}
      className={cx(
        'gh-toggle press relative inline-flex h-[1.2857rem] w-[2.2857rem] shrink-0 items-center rounded-full border',
        checked ? 'border-aqua-strong bg-aqua' : 'border-line-input bg-line',
        'disabled:cursor-not-allowed disabled:opacity-60 aria-disabled:opacity-60',
      )}
    >
      <span
        aria-hidden="true"
        // Slides 1rem with a transform when on (index.css .gh-toggle-knob), never `left`.
        className={cx(
          'gh-toggle-knob absolute left-0.5 h-3 w-3 rounded-full border bg-surface',
          checked ? 'border-aqua-strong' : 'border-line-input',
        )}
      />
    </button>
  );
}
