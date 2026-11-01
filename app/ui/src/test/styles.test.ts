import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import postcss from 'postcss';
import tailwindcss from 'tailwindcss';
import { describe, expect, it } from 'vitest';
import config from '../../tailwind.config';

// Checks that need the real CSS (jsdom has no layout and `css: false`).
const ROOT = join(__dirname, '..');

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return f === '__tests__' || f === 'test' ? [] : sources(p);
    return /\.tsx?$/.test(f) ? [p] : [];
  });
}

async function compile(raw: string): Promise<string> {
  const res = await postcss([
    tailwindcss({ ...config, content: [{ raw, extension: 'tsx' }] }),
  ]).process('@tailwind utilities;', { from: undefined });
  return res.css;
}

const indexCss = readFileSync(join(ROOT, 'styles', 'index.css'), 'utf8');

describe('Tailwind theme', () => {
  it('max-w-dialog exists and is 420 px (30 rem at the 14 px root), Design §7.13', async () => {
    const css = await compile('max-w-dialog');
    expect(css).toMatch(/\.max-w-dialog\s*\{\s*max-width:\s*30rem;?\s*\}/);
  });

  it('every named size class used in the UI is generated (no silent no-op class)', async () => {
    const code = sources(ROOT)
      .map((f) => readFileSync(f, 'utf8'))
      .join('\n');
    const named = new Set(
      [
        ...code.matchAll(
          /\b((?:max-w|min-w|w|max-h|min-h|h)-(?:dialog|content|tooltip|sidebar|tray))\b/g,
        ),
      ].map((m) => m[1]!),
    );
    expect(named.size).toBeGreaterThan(0);
    const css = await compile([...named].join(' '));
    for (const cls of named) expect(css, cls).toContain(`.${cls}`);
  });
});

describe('index.css', () => {
  it('forced colours: selected Segmented option and filled buttons stay visible (D-13)', () => {
    const fc = indexCss.slice(indexCss.indexOf('@media (forced-colors: active)'));
    expect(fc).toMatch(/\.gh-segment\[aria-checked='true'\]\s*\{[^}]*background:\s*Highlight/);
    expect(fc).toMatch(/\.gh-segment\[aria-checked='true'\]\s*\{[^}]*color:\s*HighlightText/);
    expect(fc).toMatch(/\.gh-btn\s*\{[^}]*border:\s*1px solid ButtonText/);
  });

  it('tables stack rows in a narrow container instead of scrolling sideways (D-09)', () => {
    expect(indexCss).toMatch(/\.gh-table\s*\{\s*container-type:\s*inline-size;/);
    expect(indexCss).toMatch(/@container \(max-width: \d+rem\)/);
  });

  it('programmatically focused headings draw no stray focus box (D-15)', () => {
    expect(indexCss).toMatch(/h1\[tabindex='-1'\]:focus/);
  });
});

describe('Tooltip', () => {
  it('uses small 400, not 500 (Design §7.15)', () => {
    const t = readFileSync(join(ROOT, 'components', 'Tooltip.tsx'), 'utf8');
    expect(t).toMatch(/text-small text-white/);
    expect(t).not.toMatch(/gh-tooltip[^']*font-medium/);
  });
});
