import { cx } from '../lib/cx';
import { initial } from '../lib/format';
import type { ProfileColor } from '../lib/types';
import { S } from '../strings';

export const PROFILE_BG: Record<ProfileColor, string> = {
  blue: 'bg-profile-blue',
  violet: 'bg-profile-violet',
  pink: 'bg-profile-pink',
  indigo: 'bg-profile-indigo',
  plum: 'bg-profile-plum',
  slate: 'bg-profile-slate',
  navy: 'bg-profile-navy',
  steel: 'bg-profile-steel',
};

const SIZE = {
  8: 'h-2 w-2',
  22: 'h-[1.5714rem] w-[1.5714rem] text-[0.7143rem]',
  24: 'h-6 w-6 text-[0.7714rem]',
  30: 'h-[2.1429rem] w-[2.1429rem] text-[0.9643rem]',
  40: 'h-10 w-10 text-[1.2857rem]',
} as const;

/**
 * Design §7.4 — profile colour circle with white initial (~45% of diameter).
 * Accessible name includes the colour name: "Personal, violet". Size 8 is the
 * plain dot used in the sidebar Default panel.
 */
export function Avatar({
  label,
  color,
  size = 30,
  decorative = false,
  className,
}: {
  label: string;
  color: ProfileColor;
  size?: keyof typeof SIZE;
  /** Hide from assistive tech when the label is already next to it. */
  decorative?: boolean;
  className?: string;
}) {
  const name = S.avatar.name(label, S.colors[color]);
  return (
    <span
      role={decorative ? undefined : 'img'}
      aria-label={decorative ? undefined : name}
      aria-hidden={decorative || undefined}
      className={cx(
        'gh-avatar transition-colour-base inline-flex shrink-0 select-none items-center justify-center rounded-full font-medium leading-none text-white',
        PROFILE_BG[color],
        SIZE[size],
        className,
      )}
    >
      {size === 8 ? null : initial(label)}
    </span>
  );
}
