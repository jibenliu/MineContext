// 更新检查适配层：对照 GitHub Releases 的结果形状与打开链接命令。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createUpdateCheck } from '../update-check.ts'

type Call = { command: string; args?: Record<string, unknown> }

function fakeTauri(result: unknown): { calls: Call[]; target: unknown } {
  const calls: Call[] = []
  return {
    calls,
    target: {
      __TAURI__: {
        core: {
          invoke: async (command: string, args?: Record<string, unknown>) => {
            calls.push({ command, args })
            if (command === 'check_for_update') return result
            return undefined
          }
        }
      }
    }
  }
}

test('非 Tauri 环境：shell=none，检查与打开都明确失败', async () => {
  const api = createUpdateCheck({})
  assert.equal(api.shell, 'none')
  await assert.rejects(() => api.check(), /需要桌面外壳/)
  await assert.rejects(() => api.openUrl('https://example.com'), /需要桌面外壳/)
})

test('有更新时透传 updateInfo / currentVersion', async () => {
  const { calls, target } = fakeTauri({
    updateInfo: {
      version: '1.0.8',
      htmlUrl: 'https://github.com/jibenliu/MineContext/releases/tag/v1.0.8',
      dmgUrl: 'https://example.com/MineContext_1.0.8_aarch64.dmg'
    },
    currentVersion: '1.0.7'
  })
  const api = createUpdateCheck(target)
  assert.equal(api.shell, 'tauri')

  const got = await api.check()
  assert.equal(calls[0]?.command, 'check_for_update')
  assert.equal(got.currentVersion, '1.0.7')
  assert.equal(got.updateInfo?.version, '1.0.8')
  assert.equal(
    got.updateInfo?.dmgUrl,
    'https://example.com/MineContext_1.0.8_aarch64.dmg'
  )
})

test('已是最新：updateInfo 为 null', async () => {
  const { target } = fakeTauri({ updateInfo: null, currentVersion: '1.0.7' })
  const got = await createUpdateCheck(target).check()
  assert.equal(got.updateInfo, null)
  assert.equal(got.currentVersion, '1.0.7')
})

test('打开链接走 open_external_url', async () => {
  const { calls, target } = fakeTauri({ updateInfo: null, currentVersion: '1.0.7' })
  await createUpdateCheck(target).openUrl('https://github.com/jibenliu/MineContext/releases')
  assert.equal(calls[0]?.command, 'open_external_url')
  assert.deepEqual(calls[0]?.args, {
    url: 'https://github.com/jibenliu/MineContext/releases'
  })
})
