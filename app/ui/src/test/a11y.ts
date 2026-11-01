import axe from 'axe-core';
import { expect } from 'vitest';

/**
 * axe-core check. Colour contrast can't be computed in jsdom (it's verified in
 * Design §2.3); "region" is off because components render outside landmarks.
 */
export async function expectNoA11yViolations(node: Element = document.body): Promise<void> {
  const res = await axe.run(node, {
    rules: {
      'color-contrast': { enabled: false },
      region: { enabled: false },
      // Experimental in axe, off by default: QA found D-14 with it (WCAG 2.5.3).
      'label-content-name-mismatch': { enabled: true },
    },
  });
  const summary = res.violations.map(
    (v) => `${v.id}: ${v.help}\n  ${v.nodes.map((n) => n.html).join('\n  ')}`,
  );
  expect(summary).toEqual([]);
}
