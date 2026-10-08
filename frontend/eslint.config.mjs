import eslint from '@eslint/js'
import eslintReact from '@eslint-react/eslint-plugin'
import { defineConfig } from 'eslint/config'
import eslintConfigPrettier from 'eslint-config-prettier'
import eslintPluginPrettier from 'eslint-plugin-prettier'
import reactHooks from 'eslint-plugin-react-hooks'
import simpleImportSort from 'eslint-plugin-simple-import-sort'
import unusedImports from 'eslint-plugin-unused-imports'
import globals from 'globals'
import tseslint from 'typescript-eslint'

export default defineConfig([
  eslint.configs.recommended,
  tseslint.configs.recommended,
  {
    // 与 `eslint-plugin-prettier/recommended` 等价：注册插件 + 关掉与 prettier 冲突的规则
    plugins: { prettier: eslintPluginPrettier },
    rules: { ...eslintConfigPrettier.rules }
  },
  eslintReact.configs['recommended-typescript'],
  reactHooks.configs['recommended-latest'],
  {
    plugins: {
      'simple-import-sort': simpleImportSort,
      'unused-imports': unusedImports
    },
    rules: {
      '@typescript-eslint/explicit-function-return-type': 'off',
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-non-null-asserted-optional-chain': 'off',
      'simple-import-sort/imports': 'error',
      'simple-import-sort/exports': 'error',
      'unused-imports/no-unused-imports': 'error',
      '@eslint-react/no-prop-types': 'error',
      'prettier/prettier': ['error']
    }
  },
  // Configuration for ensuring compatibility with the original ESLint(8.x) rules
  {
    rules: {
      '@typescript-eslint/no-require-imports': 'off',
      '@typescript-eslint/no-unused-vars': ['error', { caughtErrors: 'none' }],
      '@typescript-eslint/no-unused-expressions': 'off',
      '@typescript-eslint/no-empty-object-type': 'off',
      '@eslint-react/hooks-extra/no-direct-set-state-in-use-effect': 'off',
      '@eslint-react/web-api/no-leaked-event-listener': 'off',
      '@eslint-react/web-api/no-leaked-timeout': 'off',
      '@eslint-react/no-unknown-property': 'off',
      '@eslint-react/no-nested-component-definitions': 'off',
      '@eslint-react/dom/no-dangerously-set-innerhtml': 'off',
      '@eslint-react/no-array-index-key': 'off',
      '@eslint-react/no-unstable-default-props': 'off',
      '@eslint-react/no-unstable-context-value': 'off',
      '@eslint-react/hooks-extra/prefer-use-state-lazy-initialization': 'off',
      '@eslint-react/hooks-extra/no-unnecessary-use-prefix': 'off',
      '@eslint-react/no-children-to-array': 'off'
    }
  },
  {
    // LoggerService Custom Rules - only apply to src directory
    files: ['src/**/*.{ts,tsx,js,jsx}'],
    ignores: ['src/**/__tests__/**', 'src/**/__mocks__/**', 'src/**/*.test.*'],
    rules: {
      // 调试用的 console.log 不许进主干（含用户内容的更是）：统一走 @shared/logger，
      // 它才会带上 scope 并落到 renderer.log。warn/error/info/debug 仍允许 ——
      // 适配层与 logger 自己在用（logger 在 packages/ 下，不受这条约束）。
      'no-console': ['error', { allow: ['warn', 'error', 'info', 'debug'] }],
      'no-restricted-syntax': [
        process.env.PRCI ? 'error' : 'warn',
        {
          selector: 'CallExpression[callee.object.name="console"]',
          message: '统一走 @shared/logger（packages/shared/logger）：它才会带上 scope 并落到 renderer.log。'
        }
      ]
    }
  },
  {
    files: ['**/*.{ts,tsx,js,jsx,mjs,cjs}'],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: {
        ...globals.browser,
        ...globals.node
      }
    },
    plugins: {
      i18n: {
        rules: {
          'no-template-in-t': {
            meta: {
              type: 'problem',
              docs: {
                description: '⚠️不建议在 t() 函数中使用模板字符串，这样会导致渲染结果不可预料',
                recommended: true
              },
              messages: {
                noTemplateInT: '⚠️不建议在 t() 函数中使用模板字符串，这样会导致渲染结果不可预料'
              }
            },
            create(context) {
              return {
                CallExpression(node) {
                  const { callee, arguments: args } = node
                  const isTFunction =
                    (callee.type === 'Identifier' && callee.name === 't') ||
                    (callee.type === 'MemberExpression' &&
                      callee.property.type === 'Identifier' &&
                      callee.property.name === 't')

                  if (isTFunction && args[0]?.type === 'TemplateLiteral') {
                    context.report({
                      node: args[0],
                      messageId: 'noTemplateInT'
                    })
                  }
                }
              }
            }
          }
        }
      }
    },
    rules: {
      'i18n/no-template-in-t': 'warn'
    }
  },
  {
    ignores: [
      'node_modules/**',
      'build/**',
      'dist/**',
      'out/**',
      'local/**',
      '.yarn/**',
      '.gitignore',
      'scripts/cloudflare-worker.js'
    ]
  }
])
