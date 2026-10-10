// daemon 被 kill / 端口变更后，外壳 `get_runtime` 会自愈并写出新 port/token。
// 已装好的 HTTP/SSE 适配层仍指向旧端口时，必须按新 runtime 重装，不能升硬错误。

import type { BootstrapRuntime } from '../bootstrap-backend.ts'

export type HealResult = 'reclaimed' | 'unchanged' | 'unavailable'

export function runtimeIdentityChanged(
  previous: BootstrapRuntime | null | undefined,
  next: BootstrapRuntime | null | undefined
): boolean {
  if (!previous || !next) return previous != null || next != null
  return previous.port !== next.port || previous.token !== next.token
}

/**
 * 在断开后向外壳要 runtime 并重装适配层。
 *
 * - 读到与 previous 不同的 port/token → install 后 `reclaimed`
 * - 读到相同身份 → `unchanged`（不必重装）
 * - 一直读不到 → `unavailable`（调用方再决定是否升 failed）
 */
export async function reclaimBackend(options: {
  loadRuntime: () => Promise<BootstrapRuntime | null>
  install: (runtime: BootstrapRuntime) => void
  previous?: BootstrapRuntime | null
  retry?: { attempts: number; delayMs: number }
  sleep?: (ms: number) => Promise<void>
}): Promise<HealResult> {
  const retry = options.retry ?? { attempts: 40, delayMs: 250 }
  const sleep = options.sleep ?? ((ms: number) => new Promise((resolve) => setTimeout(resolve, ms)))
  const attempts = Math.max(1, retry.attempts)
  const previous = options.previous ?? null

  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    const runtime = await options.loadRuntime()
    if (runtime) {
      if (!runtimeIdentityChanged(previous, runtime)) {
        return 'unchanged'
      }
      options.install(runtime)
      return 'reclaimed'
    }
    if (attempt < attempts) await sleep(retry.delayMs)
  }
  return 'unavailable'
}
