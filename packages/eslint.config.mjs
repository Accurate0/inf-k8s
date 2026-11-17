import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['**/dist/**', '**/node_modules/**', 'feature-flag-client/**'] },
  {
    files: ['**/*.{ts,mts,mjs}'],
    languageOptions: { parser: tseslint.parser },
    rules: { curly: ['error', 'all'] },
  },
);
