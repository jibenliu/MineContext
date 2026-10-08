// 适配层的公共类型。
//
// 设计要点：渲染层直接使用 `window.dbAPI` / `window.screenMonitorAPI` 等全局，
// 我们**不改这些调用点**，而是由适配层决定这些全局怎么拿到数据：
//   HttpBackend（唯一实现：走 mc-daemon 的 HTTP + SSE）
// 与桌面外壳渠道的渠道名兼容由 `ipc-renderer-shim.ts` 负责，与本文件无关。

/** 一次请求-响应式调用（对应 ipcRenderer.invoke）。 */
export interface Backend {
  readonly kind: 'http'
  invoke<T>(channel: string, ...args: unknown[]): Promise<T>
  /** 订阅推送（对应 ipcRenderer.on），返回取消订阅函数。 */
  subscribe(event: string, handler: (payload: unknown) => void): () => void
}

export interface HttpRequest {
  method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  path: string
  body?: unknown
}

/** `runtime.json` 的内容（daemon 启动时写出）。 */
export interface RuntimeInfo {
  port: number
  token: string
  pid: number
  version: string
  started_at: string
}

export interface FetchResponseLike {
  status: number
  ok: boolean
  text(): Promise<string>
  /** 流式响应（事件流 / 对话流）才有；测试里可以构造假流。 */
  body?: ReadableStream<Uint8Array> | null
}

export type FetchLike = (
  url: string,
  init?: {
    method?: string
    headers?: Record<string, string>
    body?: string
    /** 主动关闭长连接（事件流）用。 */
    signal?: AbortSignal
  }
) => Promise<FetchResponseLike>

export interface StreamHandle {
  close(): void
}

/**
 * 打开一条 SSE 流。抽成可注入的函数，测试里可以构造假流、
 * 模拟断线与事件注入，而不用起真实端口。
 */
export type StreamFactory = (onEvent: (event: string, payload: unknown) => void, onClose?: () => void) => StreamHandle

/** daemon 返回的业务错误（保留机器可读错误码与补救建议）。 */
export interface BackendError extends Error {
  errorCode?: string
  remediation?: string
  status?: number
}
