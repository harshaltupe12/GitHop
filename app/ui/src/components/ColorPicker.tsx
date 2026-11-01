import { useRef, type KeyboardEvent } from 'react';
import { cx } from '../lib/cx';
import { PROFILE_COLORS, type ProfileColor } from '../lib/types';
import { S } from '../strings';
import { PROFILE_BG } from './Avatar';
import { Ico, Icons } from './icons';
import { Tooltip } from './Tooltip';

/**
 * Design §7.10 — row of 8 swatches (24 px). Selected: 2 px `text` ring +
 * checkmark. Colours used by another profile are disabled with a tooltip
 * "Used by Office". Arrow keys move the selection (radio group).
 */
export function ColorPicker({
  value,
  onChange,
  takenBy,
  labelledBy,
  describedBy,
}: {
  value: ProfileColor;
  onChange: (c: ProfileColor) => void;
  /** colour → label of the profile using it */
  takenBy: Partial<Record<ProfileColor, string>>;
  labelledBy: string;
  describedBy?: string;
}) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const free = PROFILE_COLORS.filter((c) => !takenBy[c] || c === value);

  const move = (e: KeyboardEvent, dir: 1 | -1) => {
    e.preventDefault();
    const i = free.indexOf(value);
    const next = free[(i + dir + free.length) % free.length];
    if (next) {
      onChange(next);
      refs.current[PROFILE_COLORS.indexOf(next)]?.focus();
    }
  };

  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      className="flex flex-wrap gap-2"
    >
      {PROFILE_COLORS.map((c, i) => {
        const used = takenBy[c] && c !== value ? takenBy[c] : undefined;
        const selected = c === value;
        const name = S.colors[c];
        const swatch = (
          <button
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            aria-disabled={used ? true : undefined}
            aria-label={used ? `${name}, ${S.form.usedBy(used).toLowerCase()}` : name}
            tabIndex={selected ? 0 : -1}
            onClick={() => !used && onChange(c)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowRight' || e.key === 'ArrowDown') move(e, 1);
              else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') move(e, -1);
            }}
            className={cx(
              'gh-avatar transition-ui inline-flex h-6 w-6 items-center justify-center rounded-full text-white',
              PROFILE_BG[c],
              selected && 'ring-2 ring-ink ring-offset-2 ring-offset-surface',
              used && 'cursor-not-allowed opacity-40',
            )}
          >
            {selected ? <Ico icon={Icons.ok} /> : null}
          </button>
        );
        return used ? (
          <Tooltip key={c} content={S.form.usedBy(used)}>
            {swatch}
          </Tooltip>
        ) : (
          <span key={c} className="inline-flex">
            {swatch}
          </span>
        );
      })}
    </div>
  );
}
