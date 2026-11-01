export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(' ');
}

let seq = 0;
/** Stable-enough unique ids for label/description wiring (React 18 has useId too). */
export const uid = (prefix = 'gh') => `${prefix}-${++seq}`;
