import { describe, expect, it } from 'vitest';
import { suggestLabel } from '../format';

describe('suggestLabel', () => {
  it('names a company identity after its email domain', () => {
    expect(suggestLabel('jane@acmelabs.com', 'Personal')).toBe('Acmelabs');
    expect(suggestLabel('jane@mail.acme-corp.co.uk', 'Personal')).toBe('Acmecorp');
  });

  it('suggests the personal label for free-mail and GitHub noreply', () => {
    expect(suggestLabel('jane@gmail.com', 'Personal')).toBe('Personal');
    expect(suggestLabel('1+jane@users.noreply.github.com', 'Personal')).toBe('Personal');
  });

  it('leaves the label empty when the email says nothing', () => {
    expect(suggestLabel(undefined, 'Personal')).toBe('');
    expect(suggestLabel('', 'Personal')).toBe('');
    expect(suggestLabel('not-an-email', 'Personal')).toBe('');
  });
});
