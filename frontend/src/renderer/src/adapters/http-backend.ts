// HttpBackend：渠道 → mc-daemon 的 HTTP 请求；订阅 → SSE。
//
// 所有请求都带 `X-MC-Token`（健康检查除外）。

import { resolveChannel, subscriptionEvent } from './channel-map.ts'
import type { Backend, BackendError, FetchLike, RuntimeInfo, StreamFactory, StreamHandle } from './types.ts'

export interface HttpBackendOptions {
  runtime: RuntimeInfo
  fetch: FetchLike
  /** 注入 SSE 工厂；测试里用假流，生产用 `fetch` + `ReadableStream` 封装。 */
  streamFactory?: StreamFactory
  /** 重连上限，防止 daemon 长期不可用时无限重连。 */
  maxReconnects?: number
}

function makeError(message: string, extra: Partial<BackendError> = {}): BackendError {
  const error = new Error(message) as BackendError
  Object.assign(error, extra)
  return error
}

export function createHttpBackend(options: HttpBackendOptions): Backend {
  const base = `http://127.0.0.1:${options.runtime.port}`
  const { fetch, streamFactory, maxReconnects = 10 } = options

  // ---- 订阅状态：单条 SSE 流，多路分发 ----
  const handlers = new Map<string, Set<(payload: unknown) => void>>()
  let stream: StreamHandle | undefined
  let reconnects = 0

  function dispatch(event: string, payload: unknown): void {
    // 收到帧就说明连接是好的：把重连预算还回去。
    // 不复位的话，daemon 一生中累计断开 10 次之后，前端就再也不重连了 ——
    // 界面上的表现是"某些地方永远不再刷新"，没有任何提示。
    reconnects = 0
    for (const channel of handlers.keys()) {
      if (subscriptionEvent(channel) !== event) continue
      for (const handler of handlers.get(channel) ?? []) handler(payload)
    }
  }

  function openStream(): void {
    if (!streamFactory) return
    stream = streamFactory(dispatch, () => {
      stream = undefined
      if (handlers.size === 0) return
      if (reconnects >= maxReconnects) return
      reconnects += 1
      // daemon 只监听本机，立即重连是安全的；上限由 maxReconnects 兜住。
      openStream()
    })
  }

  function ensureStream(): void {
    if (!stream && handlers.size > 0) openStream()
  }

  return {
    kind: 'http',

    async invoke<T>(channel: string, ...args: unknown[]): Promise<T> {
      const request = resolveChannel(channel, args)
      if (!request) {
        throw makeError(`渠道 ${channel} 尚未映射到 HTTP 接口`, {
          errorCode: 'channel_not_mapped'
        })
      }

      const response = await fetch(`${base}${request.path}`, {
        method: request.method,
        headers: {
          'X-MC-Token': options.runtime.token,
          ...(request.body !== undefined ? { 'Content-Type': 'application/json' } : {})
        },
        body: request.body !== undefined ? JSON.stringify(request.body) : undefined
      })

      const text = await response.text()
      let envelope: {
        code?: number
        message?: string
        data?: unknown
        error_code?: string | null
        remediation?: string | null
      }
      try {
        envelope = JSON.parse(text)
      } catch {
        throw makeError(`${channel} 返回了非 JSON 响应（HTTP ${response.status}）`, {
          status: response.status,
          errorCode: 'invalid_response'
        })
      }

      if (!response.ok) {
        throw makeError(envelope.message ?? `HTTP ${response.status}`, {
          status: response.status,
          errorCode: envelope.error_code ?? 'http_error',
          remediation: envelope.remediation ?? undefined
        })
      }

      // 兼容面失败时仍返回 HTTP 200 + 非零 code（调用方不处理非 200）
      if (typeof envelope.code === 'number' && envelope.code !== 0) {
        throw makeError(envelope.message ?? '请求失败', {
          status: response.status,
          errorCode: envelope.error_code ?? 'business_error',
          remediation: envelope.remediation ?? undefined
        })
      }

      return envelope.data as T
    },

    subscribe(channel: string, handler: (payload: unknown) => void): () => void {
      const set = handlers.get(channel) ?? new Set()
      set.add(handler)
      handlers.set(channel, set)
      ensureStream()

      return () => {
        const current = handlers.get(channel)
        current?.delete(handler)
        if (current && current.size === 0) {
          handlers.delete(channel)
        }
      }
    }
  }
}
