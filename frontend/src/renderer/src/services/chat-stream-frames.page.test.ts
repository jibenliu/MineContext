// AI 助手的流式**分帧**行为。
//
// 聊天流走的是 `fetch` + `ReadableStream`（不是 EventSource），帧格式是
// `data: {json}\n\n`。这一层最容易出的问题是「一次网络读取里有多帧 /
// 一帧被切成两次读取」—— 前者会漏帧，后者会 JSON 解析失败后**静默丢弃**，
// 表现成「助手少说了半句」，而且不会报错。因此值得直接喂合成流来验。

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { configureHttpClient } from './axios-config'
import { chatStreamService } from './chat-stream-service'

const originalFetch = globalThis.fetch

beforeEach(() => {
  // 端口与 token 只能来自运行时注入（daemon 的 `runtime.json`）——
  // 服务在没有地址时**拒绝发请求**，所以这里必须先按真实契约配置一次。
  configureHttpClient(45001, 'token-from-runtime-json')
})

afterEach(() => {
  globalThis.fetch = originalFetch
})

/** 把若干字符串当作**网络分片**喂给流：分片边界故意切在一帧中间。 */
function streamOf(chunks: string[]): void {
  const encoder = new TextEncoder()
  let index = 0
  globalThis.fetch = (async () => ({
    ok: true,
    body: {
      getReader: () => ({
        read: async () => {
          if (index >= chunks.length) return { done: true, value: undefined }
          return { done: false, value: encoder.encode(chunks[index++]) }
        }
      })
    }
  })) as unknown as typeof fetch
}

async function run(): Promise<unknown[]> {
  const events: unknown[] = []
  await chatStreamService
    .sendStreamMessage(
      { query: '我今天做了什么？' } as never,
      (event: unknown) => events.push(event),
      () => {},
      () => events.push({ type: 'done' })
    )
    .catch(() => undefined)
  return events
}

describe('chat stream 分帧（5.41）', () => {
  it('一次读取里的多帧全部派发（不漏帧）', async () => {
    streamOf([
      'data: {"type":"stream_chunk","content":"上午"}\n\n' + 'data: {"type":"stream_chunk","content":"写脚本"}\n\n'
    ])

    const events = await run()

    expect(events.filter((e) => (e as { type: string }).type === 'stream_chunk')).toHaveLength(2)
    expect(events[events.length - 1]).toEqual({ type: 'done' })
  })

  it('一帧被切成两次读取时也要拼回来（不能丢半句）', async () => {
    streamOf(['data: {"type":"stream_chunk","cont', 'ent":"拼回来的内容"}\n\n'])

    const events = await run()

    const chunks = events.filter((e) => (e as { type: string }).type === 'stream_chunk')
    expect(chunks).toHaveLength(1)
    expect((chunks[0] as { content: string }).content).toBe('拼回来的内容')
  })

  it('坏帧被跳过，后面的好帧仍然派发（不能一颗坏帧毁掉整轮）', async () => {
    streamOf(['data: {这不是 JSON}\n\n' + 'data: {"type":"stream_chunk","content":"好的"}\n\n'])

    const events = await run()

    const chunks = events.filter((e) => (e as { type: string }).type === 'stream_chunk')
    expect(chunks).toHaveLength(1)
    expect((chunks[0] as { content: string }).content).toBe('好的')
  })
})
