import { cx } from '../lib/cx';

/** 16 px spinner (Design §7.1 loading). Decorative — pair it with text or aria-busy. */
export function Spinner({ className }: { className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cx(
        'spin inline-block h-4 w-4 shrink-0 rounded-full border-2 border-current border-t-transparent',
        className,
      )}
    />
  );
}
