// HttpBackend —— 渠道 → HTTP、token 鉴权、错误封装、SSE 重连。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createHttpBackend } from '../http-backend.ts'
import type { FetchLike, StreamFactory, StreamHandle } from '../types.ts'

interface Recorded {
  url: string
  method: string
  headers: Record<string, string>
  body?: string
}

function fakeFetch(response: unknown = null, status = 200) {
  const recorded: Recorded[] = []
  const fetch: FetchLike = async (url, init) => {
    recorded.push({
      url,
      method: init?.method ?? 'GET',
      headers: init?.headers ?? {},
      body: init?.body
    })
    return {
      status,
      ok: status >= 200 && status < 300,
      text: async () => JSON.stringify({ code: 0, status, message: 'ok', data: response, error_code: null })
    }
  }
  return { fetch, recorded }
}

/** 可注入事件、可模拟断线的假 SSE 流。 */
interface FakeStream extends StreamHandle {
  __push(event: string, payload: unknown): void
  __fail(): void
}

function fakeStreams() {
  const streams: FakeStream[] = []

  const factory: StreamFactory = (onEvent, onClose) => {
    const stub: FakeStream = {
      close: () => undefined,
      __push(event, payload) {
        onEvent(event, payload)
      },
      __fail() {
        onClose?.()
      }
    }
    streams.push(stub)
    return stub
  }

  return { factory, streams }
}

const RUNTIME = { port: 17331, token: 'tok-123', pid: 1, version: '0.1.0', started_at: '' }

test('channel_maps_to_expected_http_request', async () => {
  const { fetch, recorded } = fakeFetch([])
  const backend = createHttpBackend({ runtime: RUNTIME, fetch })

  await backend.invoke('database:get-vault-by-id', 42)
  await backend.invoke('database:update-vault-by-id', 42, { title: 'x' })
  await backend.invoke('database:delete-vault-by-id', 42)

  assert.equal(recorded[0].method, 'GET')
  assert.equal(recorded[0].url, 'http://127.0.0.1:17331/api/db/vaults/42')

  assert.equal(recorded[1].method, 'PATCH')
  assert.equal(recorded[1].url, 'http://127.0.0.1:17331/api/db/vaults/42')
  assert.deepEqual(JSON.parse(recorded[1].body!), { title: 'x' })

  assert.equal(recorded[2].method, 'DELETE')
})

test('token_header_is_sent_on_every_request', async () => {
  const { fetch, recorded } = fakeFetch([])
  const backend = createHttpBackend({ runtime: RUNTIME, fetch })

  await backend.invoke('database:get-all-vaults')

  assert.equal(recorded[0].headers['X-MC-Token'], 'tok-123')
})

test('envelope_data_is_unwrapped', async () => {
  const { fetch } = fakeFetch([{ id: 1 }])
  const backend = createHttpBackend({ runtime: RUNTIME, fetch })

  const result = await backend.invoke('database:get-all-vaults')
  assert.deepEqual(result, [{ id: 1 }])
})

test('business_error_envelope_is_surfaced_with_error_code', async () => {
  const fetch: FetchLike = async () => ({
    status: 200,
    ok: true,
    text: async () =>
      JSON.stringify({
        code: 1,
        status: 200,
        message: '模型服务的 API Key 无效或已过期。',
        data: null,
        error_code: 'provider_auth_failed',
        remediation: '请更新 API Key。'
      })
  })
  const backend = createHttpBackend({ runtime: RUNTIME, fetch })

  await assert.rejects(
    () => backend.invoke('database:get-all-vaults'),
    (error: Error & { errorCode?: string; remediation?: string }) => {
      assert.equal(error.errorCode, 'provider_auth_failed', '必须保留机器可读错误码')
      assert.equal(error.remediation, '请更新 API Key。')
      assert.match(error.message, /API Key/)
      return true
    }
  )
})

test('http_status_error_is_surfaced', async () => {
  const fetch: FetchLike = async () => ({
    status: 401,
    ok: false,
    text: async () => JSON.stringify({ error_code: 'config_invalid', message: '未授权' })
  })
  const backend = createHttpBackend({ runtime: RUNTIME, fetch })

  await assert.rejects(() => backend.invoke('database:get-all-vaults'), /未授权|401/)
})

test('non_json_response_is_reported_clearly', async () => {
  const fetch: FetchLike = async () => ({
    status: 500,
    ok: false,
    text: async () => '<html>oops</html>'
  })
  const backend = createHttpBackend({ runtime: RUNTIME, fetch })

  await assert.rejects(
    () => backend.invoke('database:get-all-vaults'),
    (error: Error & { errorCode?: string }) => {
      assert.equal(error.errorCode, 'invalid_response')
      return true
    }
  )
})

test('unmapped_channel_fails_loudly_instead_of_silently', async () => {
  const { fetch, recorded } = fakeFetch()
  const backend = createHttpBackend({ runtime: RUNTIME, fetch })

  await assert.rejects(
    () => backend.invoke('store-sync:subscribe'),
    (error: Error & { errorCode?: string }) => {
      assert.equal(error.errorCode, 'channel_not_mapped')
      return true
    }
  )
  assert.equal(recorded.length, 0, '未映射渠道不应发出任何请求')
})

// 0.43
test('sse_reconnect_recovers_latest_activity', () => {
  const { factory, streams } = fakeStreams()
  const backend = createHttpBackend({
    runtime: RUNTIME,
    fetch: fakeFetch(null).fetch,
    streamFactory: factory,
    maxReconnects: 3
  })

  const received: unknown[] = []
  backend.subscribe('push:latest-activity', (payload) => received.push(payload))

  assert.equal(streams.length, 1, '订阅时应建立一条流')
  streams[0].__push('push:latest-activity', { id: 1 })

  // 断开 → 自动重连
  streams[0].__fail()
  assert.equal(streams.length, 2, '断线后应自动重连')

  // 重连后的推送仍应送达同一个订阅者
  streams[1].__push('push:latest-activity', { id: 2 })

  assert.deepEqual(received, [{ id: 1 }, { id: 2 }])
})

test('reconnect_stops_at_max_attempts', () => {
  const { factory, streams } = fakeStreams()
  const backend = createHttpBackend({
    runtime: RUNTIME,
    fetch: fakeFetch(null).fetch,
    streamFactory: factory,
    maxReconnects: 2
  })

  backend.subscribe('push:latest-activity', () => {})
  assert.equal(streams.length, 1)

  streams[0].__fail()
  streams[1].__fail()
  streams[2].__fail()

  assert.equal(streams.length, 3, '达到上限后不应继续重连')
})

test('unsubscribe_stops_delivery', () => {
  const { factory, streams } = fakeStreams()
  const backend = createHttpBackend({
    runtime: RUNTIME,
    fetch: fakeFetch(null).fetch,
    streamFactory: factory
  })

  const received: unknown[] = []
  const off = backend.subscribe('push:latest-activity', (payload) => received.push(payload))

  streams[0].__push('push:latest-activity', 1)
  off()
  streams[0].__push('push:latest-activity', 2)

  assert.deepEqual(received, [1])
})

test('events_are_dispatched_only_to_matching_channel', () => {
  const { factory, streams } = fakeStreams()
  const backend = createHttpBackend({
    runtime: RUNTIME,
    fetch: fakeFetch(null).fetch,
    streamFactory: factory
  })

  const activities: unknown[] = []
  const statuses: unknown[] = []
  backend.subscribe('push:latest-activity', (p) => activities.push(p))
  backend.subscribe('push:screen-monitor-status', (p) => statuses.push(p))

  streams[0].__push('push:latest-activity', 'a')
  streams[0].__push('push:screen-monitor-status', 'b')
  streams[0].__push('push:latest-activity', 'c')

  assert.deepEqual(activities, ['a', 'c'])
  assert.deepEqual(statuses, ['b'])
})

test('no_stream_is_opened_until_someone_subscribes', () => {
  const { factory, streams } = fakeStreams()
  createHttpBackend({
    runtime: RUNTIME,
    fetch: fakeFetch(null).fetch,
    streamFactory: factory
  })

  assert.equal(streams.length, 0, '无人订阅时不应建立 SSE 连接')
})

test('init-check 启动帧在晚订阅时重放（PersistGate 外先开 SSE 不丢引导判据）', () => {
  const { factory, streams } = fakeStreams()
  const backend = createHttpBackend({
    runtime: RUNTIME,
    fetch: fakeFetch(null).fetch,
    streamFactory: factory
  })

  // 先有别的渠道打开流（模拟 ServiceProvider.powerMonitor）
  backend.subscribe('push:power-monitor', () => undefined)
  const payload = '{"data":{"components":{"llm":{"status":"ok"}}}}'
  streams[0].__push('push:init-check-data', payload)

  const received: unknown[] = []
  backend.subscribe('push:get-init-check-data', (data) => received.push(data))

  assert.deepEqual(received, [payload], '晚订阅必须立刻拿到已缓存的启动帧')
})

test('init-check 启动帧在无人订阅该渠道时也缓存，晚到的订阅者仍能拿到', () => {
  const { factory, streams } = fakeStreams()
  const backend = createHttpBackend({
    runtime: RUNTIME,
    fetch: fakeFetch(null).fetch,
    streamFactory: factory
  })

  backend.subscribe('push:power-monitor', () => undefined)
  const payload = '{"data":{"components":{"llm":{"status":"unconfigured"}}}}'
  streams[0].__push('push:init-check-data', payload)

  const first: unknown[] = []
  const second: unknown[] = []
  backend.subscribe('push:get-init-check-data', (data) => first.push(data))
  backend.subscribe('push:get-init-check-data', (data) => second.push(data))

  assert.deepEqual(first, [payload])
  assert.deepEqual(second, [payload], '每个晚订阅者都应立刻收到同一份缓存帧')
})
