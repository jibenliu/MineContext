// 控制面事件流（SSE）的读取实现：`fetch` + `ReadableStream`。
//
// 为什么不用 `EventSource`：它**不能带自定义头**，而控制面除健康检查外都要求
// `X-MC-Token`。用 EventSource 的后果是流被 401 掉、事件一个都不来，而界面上的
// 表现只是「某些推送没生效」—— 最难查的一类问题。
//
// 这一层只负责「字节 → (事件名, 负载字符串)」，负载怎么解释由调用方决定。

import { getLogger } from '../../../../packages/shared/logger/renderer.ts'
import type { FetchLike, StreamFactory, StreamHandle } from './types.ts'

const logger = getLogger('sse-stream')

export interface SseFrame {
  event: string
  data: string
}

/**
 * 解析一段完整的 SSE 文本（帧之间用空行分隔）。
 *
 * 支持：多行 `data:`（用换行拼起来）、`:` 开头的注释/心跳行、缺省事件名（`message`）。
 */
export function parseSseFrames(text: string): SseFrame[] {
  const frames: SseFrame[] = []
  // SSE 规范允许 \r\n 行尾。不规范化的话 `\r\n\r\n` 不匹配 `\n\n`，
  // 帧会一直积在缓冲里（多帧事件整条流静默失效）。
  for (const block of text.replace(/\r\n/g, '\n').split(/\n\n+/)) {
    if (block.trim() === '') continue
    let event = 'message'
    const data: string[] = []
    for (const line of block.split('\n')) {
      if (line.startsWith(':')) continue
      if (line.startsWith('event:')) {
        event = line.slice('event:'.length).trim()
        continue
      }
      if (line.startsWith('data:')) {
        data.push(line.slice('data:'.length).replace(/^ /, ''))
      }
    }
    if (data.length > 0) frames.push({ event, data: data.join('\n') })
  }
  return frames
}

/** 增量解析器：网络分块会把一帧切断，未完成的尾巴要留到下一块。 */
export function createSseParser(emit: (frame: SseFrame) => void): (chunk: string) => void {
  let buffer = ''
  return (chunk: string) => {
    buffer += chunk.replace(/\r\n/g, '\n')
    const blocks = buffer.split('\n\n')
    buffer = blocks.pop() ?? ''
    for (const block of blocks) {
      for (const frame of parseSseFrames(block)) emit(frame)
    }
  }
}

export interface SseStreamOptions {
  url: string
  token?: string
  fetch: FetchLike
  /** 收到一帧。 */
  onEvent: (event: string, data: string) => void
  /** 流结束（正常或异常）——由调用方决定要不要重连。 */
  onClose: () => void
  /** 失败时说明原因；默认写控制台。 */
  onError?: (message: string) => void
}

/**
 * 打开一条 SSE 流。返回的句柄用于主动关闭。
 *
 * 失败**必须说清楚**：401（token 不对）、非 2xx、读流中断，都会走 `onError`。
 * 静默重连会让「事件永远不来」变成无法排查的现象。
 */
export function openSseStream(options: SseStreamOptions): StreamHandle {
  const controller = new AbortController()
  const report = options.onError ?? ((message: string) => logger.warn(`[mc] ${message}`))

  void (async () => {
    try {
      const response = await options.fetch(options.url, {
        method: 'GET',
        headers: {
          accept: 'text/event-stream',
          ...(options.token ? { 'X-MC-Token': options.token } : {})
        },
        signal: controller.signal
      })

      if (!response.ok) {
        report(`事件流被拒绝：HTTP ${response.status}（${options.url}）—— 检查 token 是否已写入`)
        options.onClose()
        return
      }

      const reader = response.body?.getReader()
      if (!reader) {
        report('事件流没有可读的 body')
        options.onClose()
        return
      }

      const decoder = new TextDecoder()
      const parse = createSseParser((frame) => options.onEvent(frame.event, frame.data))

      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        if (value) parse(decoder.decode(value, { stream: true }))
      }
      options.onClose()
    } catch (error) {
      if (controller.signal.aborted) return
      report(`事件流中断：${error instanceof Error ? error.message : String(error)}`)
      options.onClose()
    }
  })()

  return {
    close: () => controller.abort()
  }
}

/** 生产接线用的工厂：把 `openSseStream` 适配成 `StreamFactory`。 */
export function createSseStreamFactory(options: {
  url: string
  token?: string
  fetch: FetchLike
  onError?: (message: string) => void
}): StreamFactory {
  return (onEvent, onClose) =>
    openSseStream({
      url: options.url,
      token: options.token,
      fetch: options.fetch,
      onEvent,
      onClose: onClose ?? (() => undefined),
      onError: options.onError
    })
}
