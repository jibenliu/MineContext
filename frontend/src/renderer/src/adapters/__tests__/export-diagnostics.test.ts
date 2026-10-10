// 诊断导出入口：两条分支（Tauri / 都没有）。用 node --test 跑。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createExportDiagnostics } from '../export-diagnostics.ts'

type Call = { command: string; args?: Record<string, unknown> }

function fakeTauri(replies: Record<string, unknown>): { calls: Call[]; target: unknown } {
  const calls: Call[] = []
  return {
    calls,
    target: {
      __TAURI__: {
        core: {
          invoke: async (command: string, args?: Record<string, unknown>) => {
            calls.push({ command, args })
            return replies[command]
          }
        }
      }
    }
  }
}

test('Tauri：调用 export_diagnostics 并返回路径', async () => {
  const { calls, target } = fakeTauri({
    export_diagnostics: {
      path: '/Users/me/Downloads/MineContext-diagnostics.zip',
      folder: '/Users/me/Downloads/MineContext-diagnostics'
    }
  })
  const api = createExportDiagnostics(target)

  assert.equal(api.shell, 'tauri')
  const result = await api.export()
  assert.equal(result.path, '/Users/me/Downloads/MineContext-diagnostics.zip')
  assert.deepEqual(calls, [{ command: 'export_diagnostics', args: undefined }])
})

test('没有外壳能力：export 明确失败，不静默成功', async () => {
  const api = createExportDiagnostics({})
  assert.equal(api.shell, 'none')
  await assert.rejects(() => api.export(), /诊断导出需要桌面外壳提供/)
})

test('形状不完整的外壳（有 __TAURI__ 但没有 invoke）不接管', () => {
  const api = createExportDiagnostics({ __TAURI__: { core: {} } })
  assert.equal(api.shell, 'none')
})
