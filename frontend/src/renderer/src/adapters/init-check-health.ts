// 引导判据的 HTTP 兜底：与 SSE `push:init-check-data` 同源（`GET /api/health`）。
//
// ServiceProvider 在 PersistGate 外先订 SSE 时，启动帧可能在 App 订阅前被消费掉；
// 即便 HttpBackend 重放，流本身建连失败时仍需要这条拉取路径，否则 `showSetting`
// 会一直停在默认 true，用户被钉在引导设置页。

export interface InitCheckHealthDeps {
  getRuntime: () => Promise<{ port: number } | null | undefined>
  fetch: typeof globalThis.fetch
}

/** 拉取与 SSE 启动帧同形的 health 负载；失败抛错，由调用方决定是否保持引导态。 */
export async function fetchInitCheckFromHealth(deps: InitCheckHealthDeps): Promise<unknown> {
  const runtime = await deps.getRuntime()
  if (!runtime?.port) {
    throw new Error('daemon 运行时端口尚未就绪，无法拉取 /api/health')
  }
  const response = await deps.fetch(`http://127.0.0.1:${runtime.port}/api/health`)
  if (!response.ok) {
    throw new Error(`/api/health 返回 HTTP ${response.status}`)
  }
  return response.json()
}
