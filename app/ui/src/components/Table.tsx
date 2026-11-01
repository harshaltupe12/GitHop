import type { ReactNode } from 'react';
import { cx } from '../lib/cx';
import { useArrivals } from '../lib/motion';

export type Column<T> = {
  key: string;
  header: ReactNode;
  render: (row: T) => ReactNode;
  className?: string;
  align?: 'left' | 'right';
};

/** Design §7.9 — surface container, 32 px header, 40 px rows, hover tint. */
export function Table<T>({
  label,
  columns,
  rows,
  rowKey,
  className,
}: {
  label: string;
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  className?: string;
}) {
  // Rows that arrive after the table first rendered fade in; the initial set never does.
  const arrived = useArrivals(rows.map(rowKey));
  return (
    <div className={cx('overflow-x-auto rounded-lg border border-line bg-surface', className)}>
      <table className="w-full border-collapse text-body" aria-label={label}>
        <thead>
          <tr className="border-b border-line">
            {columns.map((c) => (
              <th
                key={c.key}
                scope="col"
                className={cx(
                  'h-8 px-3 text-small font-medium text-ink-muted',
                  c.align === 'right' ? 'text-right' : 'text-left',
                  c.className,
                )}
              >
                {c.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr
              key={rowKey(r)}
              className={cx(
                'transition-ui border-b border-line-row last:border-b-0 hover:bg-surface-hover',
                arrived(rowKey(r)) && 'appear',
              )}
            >
              {columns.map((c) => (
                <td
                  key={c.key}
                  className={cx(
                    'h-10 px-3 py-1.5 align-middle',
                    c.align === 'right' ? 'text-right' : 'text-left',
                    c.className,
                  )}
                >
                  {c.render(r)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
