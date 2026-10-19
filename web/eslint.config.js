import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import ts from 'typescript-eslint';

const authBans = ['signInWithOtp', 'signUp', 'resetPasswordForEmail', 'signInWithOAuth', 'exchangeCodeForSession'];

export default ts.config(
  { ignores: ['build/', '.svelte-kit/', 'node_modules/', 'test-results/', 'playwright-report/', 'tests/.auth/'] },
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs['flat/recommended'],
  {
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
    rules: {
      'svelte/no-at-html-tags': 'error',
      'no-restricted-imports': ['error', { patterns: ['$env/static/private', '$env/dynamic/*'] }],
      'no-restricted-syntax': [
        'error',
        ...authBans.map((name) => ({ selector: `MemberExpression[property.name='${name}']`, message: `${name} is not allowed (password login only)` })),
        { selector: "CallExpression[callee.name='fetch']", message: 'Use apiFetch (src/lib/api/client.ts)' },
        { selector: "CallExpression[callee.object.name='globalThis'][callee.property.name='fetch']", message: 'Use apiFetch' },
        { selector: "CallExpression[callee.object.name='window'][callee.property.name='fetch']", message: 'Use apiFetch' }
      ],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      'svelte/no-navigation-without-resolve': 'off',
      'svelte/require-each-key': 'off'
    }
  },
  { files: ['**/*.svelte', '**/*.svelte.ts'], languageOptions: { parserOptions: { parser: ts.parser } } },
  { files: ['src/lib/api/client.ts', 'tests/**', 'scripts/**', '*.config.*', 'src/**/*.test.ts'], rules: { 'no-restricted-syntax': 'off' } },
  { files: ['src/**/*.test.ts', 'tests/**'], rules: { '@typescript-eslint/no-explicit-any': 'off' } }
);
