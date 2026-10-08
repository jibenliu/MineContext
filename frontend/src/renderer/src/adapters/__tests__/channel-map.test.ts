// 适配层必须覆盖 preload 真正使用的每一个渠道。
//
// `fixtures/contract/used-ipc-channels.json` 由脚本从**适配层渠道表**抽取（preload 已不再引用渠道），
// 因此这条测试会随着前端改动自动变严格 —— 新增一个 IPC 调用而忘记映射，
// 测试立刻失败，而不是等到切换后端时功能静默失效。
//
// 渠道分四类，覆盖 = 属于其中任意一类，且四类互不重叠：
//   CHANNEL_MAP         → mc-daemon 的 HTTP 请求/响应
//   SUBSCRIBED_CHANNELS → mc-daemon 的 SSE 推送
//   SHELL_CHANNELS      → 桌面外壳（Tauri）提供，不走 daemon
//   DEFERRED_CHANNELS   → 明确延后，必须写明原因

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  CHANNEL_MAP,
  DEFERRED_CHANNELS,
  NEW_API_CHANNELS,
  resolveChannel,
  SHELL_CHANNELS,
  SHELL_PUSH_CHANNELS,
  SUBSCRIBED_CHANNELS,
  subscriptionEvent
} from '../channel-map.ts'

const CONTRACT = fileURLToPath(new URL('../../../../../../fixtures/contract/used-ipc-channels.json', import.meta.url))

interface Contract {
  channels: { channel: string; source: string }[]
  unknown_references: string[]
}

const contract: Contract = JSON.parse(readFileSync(CONTRACT, 'utf8'))

const ALL_SETS: Record<string, Record<string, string>> = {
  CHANNEL_MAP: CHANNEL_MAP as unknown as Record<string, string>,
  SUBSCRIBED_CHANNELS,
  SHELL_CHANNELS,
  SHELL_PUSH_CHANNELS,
  DEFERRED_CHANNELS,
  NEW_API_CHANNELS: NEW_API_CHANNELS as unknown as Record<string, string>
}

/** 新增面渠道不在 preload 里，「preload 有没有用」这条判据对它们不适用。 */
const LEGACY_SETS = ['CHANNEL_MAP', 'SUBSCRIBED_CHANNELS', 'SHELL_CHANNELS', 'SHELL_PUSH_CHANNELS', 'DEFERRED_CHANNELS']

test('契约文件本身是健康的', () => {
  assert.ok(contract.channels.length > 40, `渠道过少：${contract.channels.length}`)
  assert.deepEqual(contract.unknown_references, [], '契约里有无法解析的渠道引用')
})

test('channel_map_covers_all_used_channels', () => {
  const missing: string[] = []
  for (const { channel } of contract.channels) {
    if (Object.values(ALL_SETS).some((set) => channel in set)) continue
    missing.push(channel)
  }
  assert.deepEqual(missing, [], `以下渠道既没有映射，也没有登记为 SSE / 外壳 / 延后：\n${missing.join('\n')}`)
})

test('延后处理的渠道必须写明原因', () => {
  for (const [channel, reason] of Object.entries(DEFERRED_CHANNELS)) {
    assert.ok(reason.length > 4, `${channel} 的延后原因过于简略：${reason}`)
  }
})

test('外壳渠道必须写明由谁提供', () => {
  for (const [channel, provider] of Object.entries(SHELL_CHANNELS)) {
    assert.ok(provider.length > 4, `${channel} 未说明由哪个外壳能力提供`)
  }
})

test('四类集合互不重叠', () => {
  const seen = new Map<string, string>()
  for (const [setName, set] of Object.entries(ALL_SETS)) {
    for (const channel of Object.keys(set)) {
      const previous = seen.get(channel)
      assert.equal(previous, undefined, `${channel} 同时出现在 ${previous} 与 ${setName}，语义冲突`)
      seen.set(channel, setName)
    }
  }
})

test('没有多余条目（前端并未使用的渠道）', () => {
  const known = new Set(contract.channels.map((c) => c.channel))
  const extra: string[] = []
  for (const [setName, set] of Object.entries(ALL_SETS)) {
    if (!LEGACY_SETS.includes(setName)) continue
    for (const channel of Object.keys(set)) {
      if (!known.has(channel)) extra.push(`${setName}: ${channel}`)
    }
  }
  assert.deepEqual(extra, [], `映射表包含前端并未使用的渠道：\n${extra.join('\n')}`)
})

test('每个 HTTP 映射都能生成合法请求', () => {
  for (const channel of Object.keys(CHANNEL_MAP)) {
    const request = resolveChannel(channel, ['a', 'b', 'c'])
    assert.ok(request, `${channel} 无法解析`)
    assert.match(request.method, /^(GET|POST|PUT|PATCH|DELETE)$/, `${channel} 的方法非法`)
    assert.ok(request.path.startsWith('/'), `${channel} 的路径必须以 / 开头：${request.path}`)
    assert.ok(!request.path.includes('undefined'), `${channel} 生成了含 undefined 的路径`)
    assert.ok(!request.path.includes('NaN'), `${channel} 生成了含 NaN 的路径`)
  }
})

test('参数不足时也不产生 undefined 路径', () => {
  for (const channel of Object.keys(CHANNEL_MAP)) {
    const request = resolveChannel(channel, [])
    assert.ok(request, `${channel} 无法解析`)
    assert.ok(!request.path.includes('undefined'), `${channel} 在无参数时生成了含 undefined 的路径：${request.path}`)
  }
})

test('未知渠道返回 undefined 而不是抛异常', () => {
  assert.equal(resolveChannel('not:a-channel', []), undefined)
})

test('路径参数会被正确编码', () => {
  const request = resolveChannel('database:get-vault-by-id', [42])
  assert.ok(request)
  assert.equal(request.path, '/api/db/vaults/42')
})

test('可选参数缺省时使用默认值语义', () => {
  // preload: getNewActivities(startTime, endTime = '2099-12-31 00:00:00')
  const request = resolveChannel('database:get-new-activities', ['2026-09-30 00:00:00'])
  assert.ok(request)
  assert.ok(decodeURIComponent(request.path).includes('2099-12-31'), `缺省结束时间应沿用默认值，实际：${request.path}`)
})

test('SSE 订阅映射到正确的 event 名', () => {
  assert.equal(subscriptionEvent('push:latest-activity'), 'push:latest-activity')
  assert.equal(subscriptionEvent('push:get-init-check-data'), 'push:init-check-data')
  assert.equal(subscriptionEvent('database:get-all-vaults'), undefined)
})

test('新增面：会话列表映射到兼容路径', () => {
  const request = resolveChannel('v1:conversations', [10])
  assert.ok(request, '新增面渠道也必须能解析（两张表都要查）')
  assert.equal(request.method, 'GET')
  assert.equal(request.path, '/api/agent/chat/conversations/list?limit=10')

  // 不传 limit 时用默认值，避免把 undefined 拼进查询串
  const fallback = resolveChannel('v1:conversations', [])
  assert.ok(fallback)
  assert.equal(fallback.path, '/api/agent/chat/conversations/list?limit=20')
})
