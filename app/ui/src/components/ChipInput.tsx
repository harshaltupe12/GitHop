import { useRef, useState, type KeyboardEvent } from 'react';
import { cx } from '../lib/cx';
import { Chip } from './Chip';
import type { ControlProps } from './Field';

/**
 * Design §7.10 chip input (owners): type and press Enter or comma to add a chip;
 * Backspace on an empty input removes the last chip. Values are lower-cased
 * (the core lower-cases owners on save).
 */
export function ChipInput({
  value,
  onChange,
  placeholder,
  control,
  normalize = (s) => s.trim().toLowerCase(),
}: {
  value: string[];
  onChange: (v: string[]) => void;
  placeholder?: string;
  control: ControlProps;
  normalize?: (s: string) => string;
}) {
  const [text, setText] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const invalid = control['aria-invalid'];

  const commit = (raw: string) => {
    const parts = raw
      .split(',')
      .map(normalize)
      .filter(Boolean)
      .filter((p) => !value.includes(p));
    if (parts.length) onChange([...value, ...new Set(parts)]);
    setText('');
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      if (text.trim()) {
        e.preventDefault();
        commit(text);
      } else if (e.key === ',') {
        e.preventDefault();
      }
    } else if (e.key === 'Backspace' && !text && value.length) {
      e.preventDefault();
      onChange(value.slice(0, -1));
    }
  };

  return (
    <div
      className={cx(
        'gh-input flex min-h-8 w-full flex-wrap items-center gap-1 rounded-md border bg-surface px-1.5 py-1',
        invalid ? 'border-blocked-text' : 'border-line-input',
      )}
    >
      <ul className="contents">
        {value.map((v) => (
          <li key={v} className="max-w-full">
            <Chip
              onRemove={() => {
                onChange(value.filter((x) => x !== v));
                inputRef.current?.focus();
              }}
            >
              {v}
            </Chip>
          </li>
        ))}
      </ul>
      <input
        ref={inputRef}
        {...control}
        value={text}
        placeholder={value.length ? undefined : placeholder}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => {
          const v = e.target.value;
          if (v.includes(',')) commit(v);
          else setText(v);
        }}
        onKeyDown={onKey}
        onBlur={() => text.trim() && commit(text)}
        className="min-w-[6rem] flex-1 bg-transparent px-1 font-mono text-mono text-ink placeholder:text-ink-muted"
      />
    </div>
  );
}
