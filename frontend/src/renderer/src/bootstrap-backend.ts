// 启动时决定「用哪个后端」，并把适配层装到 `window` 上。
//
// 两条规则：
// 1. 拿到 daemon 写的 `runtime.json`（端口 + token）→ 装 HTTP 适配层；
// 2. 拿不到 → **按 `retry` 重试**（daemon 冷启动要几秒），用尽次数后返回
//    `http-unavailable` 并报告缺什么。
//    不重试会在冷启动时渲染出一个没有任何数据的界面；把状态说成「已就绪」
//    则会让用户对着一个打不通的界面排查。

/** `runtime.json` 里前端需要的两个字段。 */
export interface BootstrapRuntime {
  port: number
  token: string
}

export type BootstrapResult = 'http' | 'http-unavailable'

export interface BootstrapOptions {
  /** 读 `<data_dir>/runtime.json`；没有（daemon 没起）返回 `null`。 */
  loadRuntime: () => Promise<BootstrapRuntime | null>
  install: (runtime: BootstrapRuntime) => void
  /** 诊断用：把「发生了什么」交给调用方（控制台日志、诊断面板都行）。 */
  onEvent?: (event: string) => void
  /**
   * 读不到运行时信息时的重试策略。
   *
   * 默认 20 次 × 250 ms ≈ 5 秒：daemon 要开库、跑迁移、绑端口，
   * 冷启动通常 1–3 秒；比这个更长说明它真的没起来。
   */
  retry?: { attempts: number; delayMs: number }
  /** 测试用：把等待换掉，避免单测真的睡 250 ms。 */
  sleep?: (ms: number) => Promise<void>
}

const DEFAULT_RETRY = { attempts: 20, delayMs: 250 }

export async function bootstrapBackend(options: BootstrapOptions): Promise<BootstrapResult> {
  const event = options.onEvent ?? (() => {})

  const retry = options.retry ?? DEFAULT_RETRY
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)))
  const attempts = Math.max(1, retry.attempts)

  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    const runtime = await options.loadRuntime()
    if (runtime) {
      options.install(runtime)
      event(`backend=http（127.0.0.1:${runtime.port}，第 ${attempt} 次读到运行时信息）`)
      return 'http'
    }
    if (attempt < attempts) await sleep(retry.delayMs)
  }

  event(
    `backend=http 但读了 ${attempts} 次都没有 runtime.json —— ` +
      'daemon 未启动或数据目录不同；' +
      '本次不安装 HTTP 后端（避免静默退回 IPC 造成「以为在用 rust 后端」）'
  )
  return 'http-unavailable'
}
