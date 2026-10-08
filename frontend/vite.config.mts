// 渲染层构建配置（纯 vite）。外壳已切到 Tauri，渲染层不再需要 electron-vite。
//
// 三处必须显式写，缺一处都只在打包后才暴露：
//   1. `root` —— 渲染层入口目录（`index.html` 在这里）；
//   2. `base: './'` —— Tauri 从自定义协议加载页面，绝对路径的资源会 404；
//   3. `build.outDir` —— 必须与 Tauri 的 `frontendDist`（`../frontend/out/renderer`）一致，
//      否则外壳打出来是空壳。

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react-swc'
import { CodeInspectorPlugin } from 'code-inspector-plugin'
import { resolve } from 'path'
import { visualizer } from 'rollup-plugin-visualizer'
import { defineConfig } from 'vite'

const isDev = process.env.NODE_ENV === 'development'

export default defineConfig({
  root: resolve('src/renderer'),
  base: './',
  build: {
    outDir: resolve('out/renderer'),
    emptyOutDir: true
  },
  // 端口写死：Tauri 的 `devUrl` 是 http://localhost:5173，端口漂移等于外壳白屏。
  server: {
    port: 5173,
    strictPort: true
  },
  resolve: {
    alias: {
      '@renderer': resolve('src/renderer/src'),
      '@shared': resolve('packages/shared'),
      '@types': resolve('src/renderer/src/types')
    }
  },
  css: {
    preprocessorOptions: {
      less: {
        javascriptEnabled: true
      }
    }
  },
  plugins: [
    tailwindcss(),
    react({}),
    // 只在开发环境下启用 CodeInspectorPlugin
    ...(isDev ? [CodeInspectorPlugin({ bundler: 'vite' })] : []),
    ...(process.env.VISUALIZER_RENDERER ? [visualizer({ open: true })] : []),
    {
      name: 'force-arco-adapter-side-effect',
      transform(code, id) {
        if (id.includes('react-19-adapter')) {
          return {
            code,
            map: null,
            moduleSideEffects: true
          }
        }
        return null
      }
    }
  ]
})
