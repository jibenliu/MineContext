// 控制面事件流的读取：SSE 解析、token 头、失败要说清楚。用 node --test 跑。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { installHttpBackendFromRuntime } from '../install.ts'
import { createServerPushApi } from '../server-push-api.ts'
import { createSseParser, createSseStreamFactory, openSseStream, parseSseFrames } from '../sse-stream.ts'
import type { Backend, FetchResponseLike } from '../types.ts'

/** 假的流式 fetch：把给定的分块按顺序喂给读取方，保持连接不关闭。 */
function streamingFetch(
  chunks: string[],
  options: { status?: number; close?: boolean } = {}
): {
  requests: { url: string; headers?: Record<string, string> }[]
  fetch: (url: string, init?: { headers?: Record<string, string> }) => Promise<FetchResponseLike>
} {
  const requests: { url: string; headers?: Record<string, string> }[] = []
  return {
    requests,
    fetch: async (url, init) => {
      requests.push({ url, headers: init?.headers })
      const status = options.status ?? 200
      const encoder = new TextEncoder()
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          for (const chunk of chunks) controller.enqueue(encoder.encode(chunk))
          if (options.close === true) controller.close()
        }
      })
      return { status, ok: status < 300, text: async () => '', body }
    }
  }
}

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0))

test('parseSseFrames：事件名 + data，多行 data 用换行拼起来', () => {
  const frames = parseSseFrames('event: push:init-check-data\ndata: {"a":1}\n\ndata: plain\n')

  assert.deepEqual(frames, [
    { event: 'push:init-check-data', data: '{"a":1}' },
    { event: 'message', data: 'plain' }
  ])
})

test('parseSseFrames：注释/心跳行被忽略，只去掉紧跟冒号的一个空格', () => {
  // 按 SSE 规范：`data: x` 去掉一个空格；`data:  x` 保留第二个空格
  const frames = parseSseFrames(': keep-alive\n\nevent: x\ndata: spaced\n\ndata:  keep\n')

  assert.deepEqual(frames, [
    { event: 'x', data: 'spaced' },
    // 没有 event 行时按 SSE 规范落到默认事件名 message
    { event: 'message', data: ' keep' }
  ])
})

test('createSseParser：被网络分块切断的帧要拼回来', () => {
  const received: string[] = []
  const parse = createSseParser((frame) => received.push(`${frame.event}=${frame.data}`))

  parse('event: a\nda')
  parse('ta: 1\n\nevent: b\ndata: 2\n\n')

  assert.deepEqual(received, ['a=1', 'b=2'])
})

test('openSseStream：带 token 请求，帧按事件名分发，流结束回调 onClose', async () => {
  const fake = streamingFetch(['event: push:init-check-data\ndata: {"ok":true}\n\n'], { close: true })
  const events: [string, string][] = []
  let closed = 0

  openSseStream({
    url: 'http://127.0.0.1:1/api/v1/stream',
    token: 'secret-token',
    fetch: fake.fetch as never,
    onEvent: (event, data) => events.push([event, data]),
    onClose: () => {
      closed += 1
    }
  })
  await tick()
  await tick()

  assert.equal(fake.requests[0].url, 'http://127.0.0.1:1/api/v1/stream')
  assert.equal(fake.requests[0].headers?.['X-MC-Token'], 'secret-token', '事件流必须带 token')
  assert.deepEqual(events, [['push:init-check-data', '{"ok":true}']])
  assert.equal(closed, 1, '流结束必须回调 onClose（由调用方决定重连）')
})

test('openSseStream：401 时明确报告（不是静默不重连）', async () => {
  const fake = streamingFetch([], { status: 401 })
  const messages: string[] = []
  let closed = 0

  openSseStream({
    url: 'http://127.0.0.1:1/api/v1/stream',
    fetch: fake.fetch as never,
    onEvent: () => undefined,
    onClose: () => {
      closed += 1
    },
    onError: (message) => messages.push(message)
  })
  await tick()
  await tick()

  assert.equal(closed, 1)
  assert.equal(messages.length, 1)
  assert.match(messages[0], /401/)
  assert.match(messages[0], /token/)
})

test('createSseStreamFactory：把解析结果交给订阅方，close 后不再回调', async () => {
  const fake = streamingFetch(['data: one\n\n', 'data: two\n\n'])
  const seen: unknown[] = []

  const factory = createSseStreamFactory({
    url: 'http://127.0.0.1:1/api/v1/stream',
    fetch: fake.fetch as never
  })
  const handle = factory((_event, payload) => seen.push(payload))
  await tick()
  await tick()
  handle.close()
  await tick()

  assert.deepEqual(seen, ['one', 'two'])
})

test('推送负载按渠道解释：init-check 透传字符串，其余解成值', async () => {
  // 直接测适配层：它决定"消费方拿到什么"
  const handlers = new Map<string, (payload: unknown) => void>()
  const backend: Backend = {
    kind: 'http',
    async invoke<T>(): Promise<T> {
      return undefined as T
    },
    subscribe(event, handler) {
      handlers.set(event, handler)
      return () => handlers.delete(event)
    }
  }
  const api = createServerPushApi(backend)

  const raw: unknown[] = []
  const parsed: unknown[] = []
  api.getInitCheckData((data) => raw.push(data))
  api.pushHomeLatestActivity((activity) => parsed.push(activity))
  api.pushScreenMonitorStatus((status) => parsed.push(status))

  handlers.get('push:get-init-check-data')?.('{"data":{"components":{"llm":{"status":"ok"}}}}')
  handlers.get('push:latest-activity')?.('{"id":"a1"}')
  handlers.get('push:screen-monitor-status')?.('"running"')

  assert.deepEqual(raw, ['{"data":{"components":{"llm":{"status":"ok"}}}}'], '消费方自己 JSON.parse，必须拿到字符串')
  assert.deepEqual(parsed, [{ id: 'a1' }, 'running'], '其余渠道要的是值')
})

test('生产接线会真的打开事件流（少这一步，所有推送渠道都是死的）', async () => {
  const fake = streamingFetch(['event: push:latest-activity\ndata: {"id":"a9"}\n\n'])
  const received: unknown[] = []

  // 生产接线函数：只注入 fetch，其余用真实实现（含 SSE 工厂）
  installHttpBackendFromRuntime({ port: 6553, token: 'tok-123' }, { fetch: fake.fetch as never })

  const globals = globalThis as unknown as {
    serverPushAPI: { pushHomeLatestActivity(callback: (value: unknown) => void): () => void }
  }
  const unsubscribe = globals.serverPushAPI.pushHomeLatestActivity((value) => received.push(value))
  await tick()
  await tick()
  unsubscribe()

  const streamRequest = fake.requests.find((request) => request.url.includes('/api/v1/stream'))
  assert.ok(streamRequest, `没有打开事件流，请求只有：${JSON.stringify(fake.requests)}`)
  assert.equal(streamRequest.headers?.['X-MC-Token'], 'tok-123')
  assert.deepEqual(received, [{ id: 'a9' }])
})
