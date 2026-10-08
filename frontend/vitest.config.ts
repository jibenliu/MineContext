// 页面级测试（真实渲染层）用的 vitest 配置。
//
// 与适配层测试的分工：
// - 适配层测试（`__tests__/*.test.ts`）用 `node --test` 跑，不需要 DOM，也不需要依赖；
// - **页面级测试**（`*.page.test.tsx`）用 vitest + jsdom 跑，真的挂载 React 组件，
//   验证「适配层装到 window 上之后，页面能渲染出后端给的数据」。
//
// 目录与后缀刻意区分开：`node --test` 的 glob 只吃 `__tests__/*.test.ts`，
// 这里的 `include` 只吃 `*.page.test.tsx`，两边不会互相误伤。

import react from '@vitejs/plugin-react-swc'
import { resolve } from 'path'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  plugins: [react({})],
  resolve: {
    alias: {
      // 组件里有 `/src/assets/...` 这类**以项目根为基准**的绝对导入
      // （Vite 构建时按 root 解析），测试里要给出同样的规则，否则整个组件
      // 在收集阶段就报「找不到文件」
      // 只收 `/src/assets/**`：组件里唯一一类以项目根为基准的绝对导入。
      // 不要写成 `/src` 前缀别名 —— 那会连 `@renderer/...` 解析出来的路径一起改写。
      ['/src/assets']: resolve('src/renderer/src/assets'),
      '@renderer': resolve('src/renderer/src'),
      '@shared': resolve('packages/shared'),
      '@types': resolve('src/renderer/src/types')
    }
  },
  test: {
    environment: 'jsdom',
    include: ['src/renderer/src/**/*.page.test.{ts,tsx}'],
    setupFiles: ['src/renderer/src/test/page-setup.ts'],
    // 组件里 import 的 css / less 不参与断言，跳过处理省时间
    css: false,
    globals: true,
    restoreMocks: true
  }
})
