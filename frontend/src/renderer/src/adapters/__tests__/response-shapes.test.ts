// 响应形状契约（前端一侧）：解包函数必须能吃下**真实样例**。
//
// 与 daemon 侧的分工：
//   - `crates/mc-server/tests/response_shapes.rs` 断言 fixture 的形状 == daemon 真实返回；
//   - 这里断言前端的解包/消费能吃下 fixture 里的样例，页面测试的 mock 也从这里取。
// 两边任一漂移都会红，而不是等到界面上白屏（summary 卡片就是这么白过一次的）。

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { shouldShowOnboarding } from '../onboarding.ts'
import {
  unwrapConversations,
  unwrapSearchResponse,
  unwrapSearchResults,
  unwrapSummaries
} from '../unpack.ts'

const FIXTURE = fileURLToPath(new URL('../../../../../../fixtures/contract/response-shapes.json', import.meta.url))

interface ShapeDocument {
  channels: Record<string, { sample: unknown }>
}

const document: ShapeDocument = JSON.parse(readFileSync(FIXTURE, 'utf8'))
const sample = (channel: string): unknown => {
  const entry = document.channels[channel]
  assert.ok(entry, `fixture 里缺少 ${channel} 的样例`)
  return entry.sample
}

test('总结列表：解包成数组，且时间字段是字符串（卡片按 HH:mm 解析）', () => {
  const rows = unwrapSummaries(sample('v1:summaries'))

  assert.equal(rows.length, 1, '样例里应有一条总结')
  const row = rows[0] as Record<string, unknown>
  assert.equal(typeof row.id, 'string')
  assert.equal(typeof row.body_markdown, 'string')
  assert.equal(typeof row.quality, 'string')
  // 卡片用 `clock(summary.start)` 取 HH:mm：字段必须是字符串，数字会渲染成空
  assert.equal(typeof row.start, 'string', 'start 必须是字符串（前端按字符串读的时间格式）')
  assert.equal(typeof row.end, 'string')
})

test('总结列表：容忍裸数组（另一种可接受形状）与坏数据', () => {
  assert.deepEqual(unwrapSummaries([{ id: 'x' }]), [{ id: 'x' }])
  assert.deepEqual(unwrapSummaries({ summaries: 'not-an-array' }), [])
  assert.deepEqual(unwrapSummaries(null), [])
})

test('对话列表：{ items, total } 且 total 是数字', () => {
  const page = unwrapConversations(sample('v1:conversations'))

  assert.equal(page.items.length, 1)
  assert.equal(typeof page.total, 'number')
  const item = page.items[0] as Record<string, unknown>
  assert.equal(typeof item.id, 'number')
  assert.equal(typeof item.title, 'string')
})

test('检索结果：解包成数组，元素带 UI 用到的字段', () => {
  const results = unwrapSearchResults(sample('v1:search'))

  assert.equal(results.length, 1)
  const hit = results[0] as Record<string, unknown>
  assert.equal(typeof hit.id, 'string')
  assert.equal(typeof hit.kind, 'string')
  assert.equal(typeof hit.snippet, 'string')
  assert.equal(typeof hit.score, 'number')
  // 旧的 `{ results }` 与裸数组都要能吃
  assert.deepEqual(unwrapSearchResults([{ id: 'a' }]), [{ id: 'a' }])
  assert.deepEqual(unwrapSearchResults({ results: 'nope' }), [])
})

test('检索响应：保留 mode 供「仅本地」角标', () => {
  const payload = unwrapSearchResponse(sample('v1:search'))
  assert.equal(payload.mode, 'keyword')
  assert.equal(payload.results.length, 1)
})

test('启动握手：样例驱动的引导页判据（未配置 → 引导页）', () => {
  const payload = sample('push:init-check-data') as {
    data: { components: { llm: { status: string } } }
  }

  assert.equal(shouldShowOnboarding(payload), true, '样例是未配置模型 → 应进引导页')

  const configured = structuredClone(payload)
  configured.data.components.llm.status = 'ok'
  assert.equal(shouldShowOnboarding(configured), false, '配好了就直接进主界面')
})

test('启动状态：样例是 status=running（渲染层据此进主界面）', () => {
  const status = sample('backend:get-status') as { status?: unknown }

  assert.equal(status.status, 'running')
})

test('任意时段总结预览：counts 四个键与 range 都是 UI 直接读的', () => {
  const preview = sample('v1:adhoc-preview') as {
    range?: Record<string, unknown>
    counts?: Record<string, unknown>
    estimated_chunks?: unknown
    has_data?: unknown
  }

  for (const key of ['observations', 'blocked_observations', 'activities', 'stages']) {
    assert.equal(typeof preview.counts?.[key], 'number', `counts.${key} 必须是数字`)
  }
  assert.equal(typeof preview.range?.from, 'string')
  assert.equal(typeof preview.range?.to, 'string')
  assert.equal(typeof preview.estimated_chunks, 'number')
  assert.equal(typeof preview.has_data, 'boolean')
})
