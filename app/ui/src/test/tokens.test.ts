import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

// CLAUDE.md rule 11 / Design §12: UI code uses tokens only. This test backs up
// the `githop/tokens-only` ESLint rule with a plain text scan.
const ROOT = join(__dirname, '..');
const DIRS = ['components', 'screens', 'tray'];

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return f === '__tests__' ? [] : files(p);
    return /\.(tsx?|css)$/.test(f) && !/\.test\./.test(f) ? [p] : [];
  });
}

const sources = [...DIRS.flatMap((d) => files(join(ROOT, d))), join(ROOT, 'App.tsx')];

describe('tokens only', () => {
  it.each(sources)('%s has no raw colours, font names or off-token shapes', (file) => {
    const text = readFileSync(file, 'utf8');
    expect(text).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
    expect(text).not.toMatch(/\brgba?\(|\bhsla?\(/);
    expect(text).not.toMatch(/IBM Plex|Segoe|Cascadia|Consolas|font-family/);
    expect(text).not.toMatch(
      /\b(?:bg|text|border|ring)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}\b/,
    );
    expect(text).not.toMatch(/\bshadow-(?:sm|md|lg|xl|2xl)\b|\brounded-(?:xl|2xl|3xl)\b/);
  });

  it('no network APIs anywhere in src', () => {
    const all = files(ROOT).filter(
      (f) => !f.endsWith('tokens.test.ts') && !f.endsWith('eslint.config.js'),
    );
    for (const f of all) {
      const text = readFileSync(f, 'utf8');
      expect(text, f).not.toMatch(
        /\bfetch\(|XMLHttpRequest|new WebSocket|EventSource|sendBeacon|https?:\/\/(?!ipc\.localhost)/,
      );
    }
  });

  it('tokens.css carries Design §12.1 verbatim and is light-only', () => {
    const css = readFileSync(join(ROOT, 'styles', 'tokens.css'), 'utf8');
    for (const line of [
      '--bg: #F4F3F6;',
      '--aqua: #4ABDAC;',
      '--blocked: #FC4A1A;',
      '--warning: #F7B733;',
      '--text-muted: #6E6A74;',
      '--profile-blue: #2F6BC9;',
      '--profile-pink: #C23A7D;',
      "--font-sans: 'IBM Plex Sans', 'Segoe UI Variable', 'Segoe UI', system-ui, sans-serif;",
      '--shadow-float: 0 8px 24px rgba(31, 29, 36, 0.12);',
      'html { font-size: 14px; }',
    ])
      expect(css).toContain(line);
    expect(css).not.toMatch(/prefers-color-scheme/);
  });
});
