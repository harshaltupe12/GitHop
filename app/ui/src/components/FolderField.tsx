import { useState } from 'react';
import { cx } from '../lib/cx';
import { normalizePath, truncateMiddle } from '../lib/format';
import { pickFile, pickFolder } from '../lib/ipc';
import { S } from '../strings';
import { Button } from './Button';
import type { ControlProps } from './Field';
import { Icons } from './icons';
import { Tooltip } from './Tooltip';

/**
 * Design §7.10 folder field: read-only path + "Choose…" (native picker via
 * plugin-dialog). `mode="file"` picks a file instead (existing SSH key).
 */
export function FolderField({
  value,
  onChange,
  control,
  pickTitle,
  mode = 'directory',
  emptyText,
  onRemove,
  removeLabel,
}: {
  value: string;
  onChange: (path: string) => void;
  control: ControlProps;
  pickTitle: string;
  mode?: 'directory' | 'file';
  emptyText?: string;
  onRemove?: () => void;
  removeLabel?: string;
}) {
  const [busy, setBusy] = useState(false);
  const choose = async () => {
    setBusy(true);
    try {
      const p = mode === 'directory' ? await pickFolder(pickTitle) : await pickFile(pickTitle);
      if (p) onChange(normalizePath(p));
    } finally {
      setBusy(false);
    }
  };
  const invalid = control['aria-invalid'];
  const shown = value ? truncateMiddle(value, 48) : (emptyText ?? '');
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Tooltip content={value} className="min-w-0 flex-1">
        <input
          {...control}
          readOnly
          value={shown}
          title={value || undefined}
          className={cx(
            'gh-input min-h-8 w-full min-w-0 rounded-md border bg-bg px-2.5 font-mono text-mono',
            value ? 'text-ink' : 'text-ink-muted',
            invalid ? 'border-blocked-text' : 'border-line-input',
          )}
        />
      </Tooltip>
      <Button
        onClick={choose}
        loading={busy}
        icon={mode === 'directory' ? Icons.folder : Icons.key}
      >
        {S.common.choose}
      </Button>
      {onRemove ? (
        <Button
          variant="ghost"
          icon={Icons.dismiss}
          label={removeLabel ?? S.common.remove(value)}
          onClick={onRemove}
        />
      ) : null}
    </div>
  );
}
