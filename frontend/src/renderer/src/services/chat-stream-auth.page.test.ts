// AI 助手的流式请求必须带鉴权。
//
// 切到 rust 后端后，`/api/agent/chat/stream` 由 daemon 提供，而它**除健康检查
// 外全部要 token**。少了 `X-MC-Token` 的表现是「AI 助手一直转圈」——
// HTTP 401 在流式封装里被吞掉，用户看不到任何错误。因此值得一条专门的测试。

import { afterEach, describe, expect, it, vi } from 'vitest'

import { configureHttpClient } from './axios-config'
import { chatStreamService } from './chat-stream-service'

const originalFetch = globalThis.fetch

afterEach(() => {
  globalThis.fetch = originalFetch
  vi.restoreAllMocks()
})

function stubFetch(): { url: string; headers: Record<string, string> }[] {
  const calls: { url: string; headers: Record<string, string> }[] = []
  globalThis.fetch = (async (url: string, init?: { headers?: Record<string, string> }) => {
    calls.push({ url: String(url), headers: init?.headers ?? {} })
    throw new Error('测试里不发真实请求')
  }) as unknown as typeof fetch
  return calls
}

describe('chat stream 鉴权（rust 后端）', () => {
  it('带上 X-MC-Token 与 daemon 端口（5.41）', async () => {
    const calls = stubFetch()
    configureHttpClient(45678, 'token-from-runtime-json')

    await chatStreamService
      .sendStreamMessage(
        { query: '我今天做了什么？' } as never,
        () => {},
        () => {},
        () => {}
      )
      .catch(() => undefined)

    expect(calls.length).toBeGreaterThan(0)
    expect(calls[0].url).toContain('127.0.0.1:45678/api/agent/chat/stream')
    expect(calls[0].headers['X-MC-Token']).toBe('token-from-runtime-json')
  })
})
