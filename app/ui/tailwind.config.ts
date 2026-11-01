import type { Config } from 'tailwindcss';

// Design §12.2, kept as written for colours, font families, radii, shadow and easing.
// Two deliberate, documented differences (see Design §3.3 "use rem, root = 14px"):
//  1. Font sizes are the §12.2 px values expressed in rem (same size at 100%,
//     but they follow the Windows text-size setting when the root size changes).
//  2. The spacing scale is the 4 px base from Design §4.1, also in rem, because
//     Tailwind's default 0.25rem step would be 3.5 px with a 14 px root.
const ROOT_PX = 14;
const rem = (px: number): string => `${+(px / ROOT_PX).toFixed(4)}rem`;

const STEPS = [
  0.5, 1, 1.5, 2, 2.5, 3, 3.5, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16, 20, 24, 28, 32, 36, 40, 44, 48,
  56, 64, 72, 80, 96,
];
const spacing: Record<string, string> = { 0: '0px', px: '1px' };
for (const n of STEPS) spacing[String(n)] = rem(n * 4);

export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    spacing,
    extend: {
      colors: {
        bg: 'var(--bg)',
        scrim: 'var(--scrim)',
        surface: {
          DEFAULT: 'var(--surface)',
          hover: 'var(--surface-hover)',
          pressed: 'var(--surface-pressed)',
        },
        line: { DEFAULT: 'var(--border)', input: 'var(--border-input)', row: 'var(--row-border)' },
        ink: {
          DEFAULT: 'var(--text)',
          muted: 'var(--text-muted)',
          disabled: 'var(--text-disabled)',
          nav: 'var(--nav-text)',
        },
        aqua: {
          DEFAULT: 'var(--aqua)',
          tint: 'var(--aqua-tint)',
          border: 'var(--aqua-border)',
          ink: 'var(--aqua-ink)',
          strong: 'var(--aqua-strong)',
          text: 'var(--aqua-text)',
          hover: 'var(--aqua-hover)',
        },
        blocked: {
          DEFAULT: 'var(--blocked)',
          tint: 'var(--blocked-tint)',
          border: 'var(--blocked-border)',
          ink: 'var(--blocked-ink)',
          text: 'var(--blocked-text)',
          hover: 'var(--blocked-tint-hover)',
          pressed: 'var(--blocked-tint-pressed)',
        },
        warning: {
          DEFAULT: 'var(--warning)',
          tint: 'var(--warning-tint)',
          border: 'var(--warning-border)',
          ink: 'var(--warning-ink)',
        },
        profile: {
          blue: 'var(--profile-blue)',
          violet: 'var(--profile-violet)',
          pink: 'var(--profile-pink)',
          indigo: 'var(--profile-indigo)',
          plum: 'var(--profile-plum)',
          slate: 'var(--profile-slate)',
          navy: 'var(--profile-navy)',
          steel: 'var(--profile-steel)',
        },
      },
      fontFamily: { sans: ['var(--font-sans)'], mono: ['var(--font-mono)'] },
      fontSize: {
        display: [rem(24), { lineHeight: rem(32), fontWeight: '500' }],
        title: [rem(20), { lineHeight: rem(28), fontWeight: '500' }],
        heading: [rem(16), { lineHeight: rem(22), fontWeight: '500' }],
        body: [rem(14), { lineHeight: rem(20) }],
        small: [rem(12), { lineHeight: rem(16) }],
        mono: [rem(13), { lineHeight: rem(18) }],
        'mono-small': [rem(12), { lineHeight: rem(16) }],
      },
      borderRadius: { sm: 'var(--radius-sm)', md: 'var(--radius-md)', lg: 'var(--radius-lg)' },
      boxShadow: { float: 'var(--shadow-float)' },
      transitionTimingFunction: { ui: 'var(--ease)' },
      // Motion: values live in tokens.css (Design §10, app/ui/MOTION.md).
      transitionDuration: {
        fast: 'var(--dur-fast)',
        base: 'var(--dur-base)',
        screen: 'var(--dur-screen)',
        exit: 'var(--dur-exit)',
      },
      width: { sidebar: rem(184), dialog: rem(420), tray: rem(260) },
      // Dialog §7.13: 420 px wide — `max-w-dialog` must exist (w-full + max width).
      maxWidth: { content: rem(760), tooltip: rem(280), dialog: rem(420) },
    },
  },
  plugins: [],
} satisfies Config;
