import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import jsxA11y from 'eslint-plugin-jsx-a11y';
import tseslint from 'typescript-eslint';

// Local rules enforcing Design §12 / CLAUDE.md rule 11: tokens only in UI code.
const HEX = /#[0-9a-fA-F]{3,8}\b/;
const FONT_NAMES =
  /\b(IBM Plex|Segoe|Cascadia|Consolas|Arial|Helvetica|Roboto|Inter|monospace|sans-serif)\b/;
const TW_DEFAULT_PALETTE =
  /\b(?:bg|text|border|ring|outline|fill|stroke|from|to|via|divide|placeholder|accent|caret)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}\b/;
const NON_TOKEN_SHAPE = /\b(?:shadow-(?:sm|md|lg|xl|2xl|inner)|rounded-(?:xl|2xl|3xl))\b/;

function checkString(context, node, value) {
  if (typeof value !== 'string') return;
  if (HEX.test(value)) {
    context.report({
      node,
      message: 'Raw hex colour — use a Design token (tokens.css / Tailwind theme).',
    });
  } else if (FONT_NAMES.test(value)) {
    context.report({ node, message: 'Font name in component — use font-sans / font-mono tokens.' });
  } else if (TW_DEFAULT_PALETTE.test(value)) {
    context.report({ node, message: 'Tailwind default palette class — use Design token colours.' });
  } else if (NON_TOKEN_SHAPE.test(value)) {
    context.report({
      node,
      message: 'Shadow/radius outside Design tokens (shadow-float, rounded-sm/md/lg/full).',
    });
  }
}

const tokensPlugin = {
  rules: {
    'tokens-only': {
      meta: { type: 'problem', schema: [] },
      create(context) {
        return {
          Literal(node) {
            checkString(context, node, node.value);
          },
          TemplateElement(node) {
            checkString(context, node, node.value.cooked);
          },
          JSXText(node) {
            checkString(context, node, node.value);
          },
        };
      },
    },
  },
};

export default tseslint.config(
  { ignores: ['dist', 'node_modules', 'coverage'] },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2021,
      globals: { ...globals.browser },
    },
    plugins: {
      'react-hooks': reactHooks,
      'jsx-a11y': jsxA11y,
      githop: tokensPlugin,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      ...jsxA11y.flatConfigs.strict.rules,
      // Scrollable regions (role="region" + aria-label) must be focusable for keyboard users.
      'jsx-a11y/no-noninteractive-tabindex': ['error', { tags: [], roles: ['tabpanel', 'region'] }],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      // No network from the UI (CLAUDE.md rule 1).
      'no-restricted-globals': [
        'error',
        { name: 'fetch', message: 'GitHop never makes network requests.' },
        { name: 'XMLHttpRequest', message: 'GitHop never makes network requests.' },
        { name: 'WebSocket', message: 'GitHop never makes network requests.' },
        { name: 'EventSource', message: 'GitHop never makes network requests.' },
      ],
      'no-restricted-properties': [
        'error',
        { object: 'window', property: 'fetch', message: 'GitHop never makes network requests.' },
        {
          object: 'navigator',
          property: 'sendBeacon',
          message: 'GitHop never makes network requests.',
        },
      ],
      'no-restricted-imports': [
        'error',
        {
          paths: [
            { name: '@tauri-apps/plugin-http', message: 'Network plugins are banned.' },
            { name: '@tauri-apps/plugin-updater', message: 'Auto-updater is banned.' },
            { name: '@tauri-apps/plugin-websocket', message: 'Network plugins are banned.' },
            { name: '@tauri-apps/plugin-upload', message: 'Network plugins are banned.' },
            { name: 'axios', message: 'Network libraries are banned.' },
          ],
        },
      ],
    },
  },
  {
    // Raw Tauri access only from the typed IPC layer (CLAUDE.md conventions).
    files: [
      'src/components/**/*.{ts,tsx}',
      'src/screens/**/*.{ts,tsx}',
      'src/tray/**/*.{ts,tsx}',
      'src/App.tsx',
    ],
    rules: {
      'githop/tokens-only': 'error',
      'no-restricted-imports': [
        'error',
        {
          patterns: [
            { group: ['@tauri-apps/*'], message: 'Call the backend through src/lib/ipc.ts only.' },
          ],
        },
      ],
    },
  },
  {
    files: ['**/*.test.{ts,tsx}', 'src/test/**'],
    languageOptions: { globals: { ...globals.node } },
    rules: { 'githop/tokens-only': 'off' },
  },
  {
    files: ['*.config.{js,ts}', 'eslint.config.js'],
    languageOptions: { globals: { ...globals.node } },
  },
);
