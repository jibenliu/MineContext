// 开机自启入口：两条分支（Tauri / 都没有）各自的行为。用 node --test 跑。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createLaunchAtLogin } from '../launch-at-login.ts'

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

test('Tauri：读系统实际值，写时带 enabled 参数并读回', async () => {
  const { calls, target } = fakeTauri({ launch_at_login: true, set_launch_at_login: false })
  const api = createLaunchAtLogin(target)

  assert.equal(api.shell, 'tauri')
  assert.equal(await api.read(), true)
  assert.equal(await api.write(false), false)

  assert.deepEqual(calls, [
    { command: 'launch_at_login', args: undefined },
    { command: 'set_launch_at_login', args: { enabled: false } }
  ])
})

test('Tauri：读回来的值就是界面要显示的（不假设写入即生效）', async () => {
  // 系统里已经是 false（例如用户手动关掉了），即使请求写 true 也要照实显示
  const { target } = fakeTauri({ launch_at_login: false, set_launch_at_login: false })
  const api = createLaunchAtLogin(target)

  assert.equal(await api.write(true), false)
})

test('没有外壳能力：写的时候明确失败，不静默成功', async () => {
  const api = createLaunchAtLogin({})

  assert.equal(api.shell, 'none')
  assert.equal(await api.read(), undefined)
  await assert.rejects(() => api.write(true), /开机自启需要桌面外壳提供/)
})

test('形状不完整的外壳（有 __TAURI__ 但没有 invoke）不接管', () => {
  const api = createLaunchAtLogin({ __TAURI__: { core: {} } })
  assert.equal(api.shell, 'none')
})
