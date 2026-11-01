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
        'gh-toggle transition-ui relative inline-flex h-[1.2857rem] w-[2.2857rem] shrink-0 items-center rounded-full border',
        checked ? 'border-aqua-strong bg-aqua' : 'border-line-input bg-line',
        'disabled:cursor-not-allowed disabled:opacity-60 aria-disabled:opacity-60',
      )}
    >
      <span
        aria-hidden="true"
        className={cx(
          'transition-ui absolute h-3 w-3 rounded-full bg-surface',
          checked
            ? 'left-[1.1429rem] border border-aqua-strong'
            : 'left-0.5 border border-line-input',
        )}
        style={{ transitionProperty: 'left, background-color' }}
      />
    </button>
  );
}
