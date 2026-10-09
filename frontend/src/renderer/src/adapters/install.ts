// 一次性把适配层装到全局对象上。
//
// 关键决定：**保留 `window.dbAPI` 等全局形状**，只替换其实现。
// 这样 30 多个业务文件（pages/home、use-vault、vault-thunk…）一行都不用改。

import { getLogger, setLogSink } from '../../../../packages/shared/logger/renderer.ts'
import { createAppApi, type ShellBridge, type ShellCapabilities } from './app-api.ts'
import { createDbApi } from './db-api.ts'
import { createEventLoop } from './event-loop.ts'
import { createFileService } from './file-service.ts'
import { createHttpBackend, type HttpBackendOptions } from './http-backend.ts'
import { createIpcRendererShim } from './ipc-renderer-shim.ts'
import { installRendererLogSink } from './renderer-log-sink.ts'
import { createScreenMonitorApi } from './screen-monitor-api.ts'
import { createServerPushApi } from './server-push-api.ts'
import { createSseStreamFactory } from './sse-stream.ts'
import { createTauriShellBridge, createTauriShellCapabilities } from './tauri-shell.ts'
import type { Backend, FetchLike, StreamFactory } from './types.ts'
import { unwrapConversations, unwrapSearchResults, unwrapSummaries } from './unpack.ts'

const bootstrapLog = getLogger('bootstrap')

export interface InstallOptions {
  backend: Backend
  shell?: ShellBridge
  /** 外壳事件/命令能力；默认从目标对象上的 `__TAURI__` 探测（测试可注入）。 */
  shellCapabilities?: ShellCapabilities
  /** 注入目标对象，便于测试（默认 globalThis）。 */
  target?: Record<string, unknown>
}

export function installAdapters(options: InstallOptions): void {
  const target = options.target ?? (globalThis as unknown as Record<string, unknown>)
  const { backend } = options

  // 逐项安装：某一项装不上（例如被 contextBridge 暴露成了只读属性）时，
  // 报告出来并继续装其余项 —— 让整个 bootstrap 因为一行赋值失败而白屏，
  // 排查成本远高于「有一项没装上」。
  const install = (name: string, value: unknown): void => {
    try {
      target[name] = value
    } catch (error) {
      bootstrapLog.error(`[mc] 适配层无法安装 ${name}（该全局可能已被只读占用）：`, error)
    }
  }

  install('dbAPI', createDbApi(backend))
  install('screenMonitorAPI', createScreenMonitorApi(backend))
  install('fileService', createFileService(backend))
  install('serverPushAPI', createServerPushApi(backend))
  install('eventLoop', createEventLoop(backend))
  // 外壳能力（事件订阅 / 命令调用）在这里定一次，供 `window.api` 使用。
  const shellCapabilities = options.shellCapabilities ?? createTauriShellCapabilities(target)
  install('api', createAppApi(backend, options.shell, shellCapabilities))
  // 屏幕监控的托盘状态上报与存储同步仍按 `window.electron.ipcRenderer` 调用：
  // shim 把这些渠道映射到 Tauri 外壳能力（不是 Electron）。
  install('electron', { ipcRenderer: createIpcRendererShim(backend, shellCapabilities) })
  // 总结卡片走它：`{ list }` 与 window.* 的既有形状一致（页面不需要知道后端是谁）
  // 解包集中在 `unpack.ts`（纯函数，用 fixtures/contract/response-shapes.json 里
  // 经 daemon 侧测试校验过的真实样例测过）：消费方只处理数组/固定形状。
  // 解包散在调用点最容易漏，而漏一处的表现是 `summaries.map is not a function` 白屏：
  // 形状不对不抛错、只让页面空白，所以集中到这里并由 fixture 钉住真实形状。
  install('summaryApi', {
    list: async () => unwrapSummaries(await backend.invoke('v1:summaries'))
  })
  install('chatApi', {
    listConversations: async (limit?: number) => unwrapConversations(await backend.invoke('v1:conversations', limit)),
    listMessages: (conversationId: number) => backend.invoke('v1:conversation-messages', conversationId),
    deleteConversation: (conversationId: number) => backend.invoke('v1:conversation-delete', conversationId)
  })
  install('searchApi', {
    query: async (text: string, start?: number, end?: number) =>
      unwrapSearchResults(await backend.invoke('v1:search', text, start, end))
  })
  // 活动来源与改名：兼容面没有这两个字段，只能走扩展面
  install('activityApi', {
    list: () => backend.invoke('v1:activities'),
    rename: (activityId: string, title: string) => backend.invoke('v1:activity-override', activityId, title),
    merge: (primaryId: string, absorbed: string[]) => backend.invoke('v1:activity-merge', primaryId, absorbed),
    split: (activityId: string, atMs: number, tailTitle: string) =>
      backend.invoke('v1:activity-split', activityId, atMs, tailTitle)
  })
  // 任意时段总结：预览 / 提交作业 / 查进度 / 取消
  install('adhocApi', {
    preview: (from: string, to: string) => backend.invoke('v1:adhoc-preview', from, to),
    submit: (from: string, to: string) => backend.invoke('v1:adhoc-job-submit', from, to),
    job: (jobId: string) => backend.invoke('v1:adhoc-job', jobId),
    cancel: (jobId: string) => backend.invoke('v1:adhoc-job-cancel', jobId)
  })
  // 补偿推断：设置页入队 + 轮询状态
  install('jobsApi', {
    enqueueBackfill: (from: string, to: string) => backend.invoke('v1:jobs-backfill', from, to),
    jobStatus: (jobId: number) => backend.invoke('v1:jobs-status', jobId)
  })
  // 链接上传：笔记树「导入链接」入口
  install('linkApi', {
    importUrl: (url: string, parentId?: number | null) =>
      backend.invoke('v1:import-link', url, parentId ?? null) as Promise<{
        id: number
        title: string
        url: string
        source_host: string
      }>
  })

  // 落盘出口：优先用调用方注入的 shellCapabilities；否则按 target 再探测一次。
  // main 已在 bootstrap 前装过一次；这里再装保证 installAdapters 单独调用时也有出口。
  if (shellCapabilities?.invoke) {
    const forward = shellCapabilities.invoke
    setLogSink((level, message) => {
      void forward('renderer_log', { level, message }).catch(() => undefined)
    })
  } else {
    installRendererLogSink(target)
  }
}

/**
 * 按 `runtime.json` 装 HTTP 后端，并把**业务层直接用的 axios 客户端**也指到 daemon。
 *
 * 后者容易被忽略：设置页（`services/Settings.ts`）走的是 `axiosConfig` 而不是
 * 适配层，只装适配层的话请求会打到未就绪的地址上，并且不带 token（401）。
 *
 * 参数可注入只为测试：生产走 `globalThis.fetch` 与真实 SSE 工厂。
 */
export function installHttpBackendFromRuntime(
  runtime: { port: number; token: string },
  deps: { fetch?: FetchLike; streamFactory?: StreamFactory } = {}
): void {
  const doFetch = deps.fetch ?? (globalThis.fetch.bind(globalThis) as unknown as FetchLike)
  installAdapters({
    backend: createHttpBackend({
      runtime: { ...runtime, pid: 0, version: 'unknown', started_at: '' },
      fetch: doFetch,
      // 事件流必须显式接线：不接的话 `openStream()` 直接返回，所有推送渠道
      // （启动握手、最新活动、采集状态、总结进度）都是死的 —— 而界面上只表现为
      // 「某些地方不刷新」，没有任何报错。
      streamFactory:
        deps.streamFactory ??
        createSseStreamFactory({
          url: `http://127.0.0.1:${runtime.port}/api/v1/stream`,
          token: runtime.token,
          fetch: doFetch,
          onError: (message) => bootstrapLog.warn(message)
        })
    }),
    // Tauri 外壳：由初始化脚本注入 `__TAURI__`/`mcRuntime`，这里探测并接管外壳能力。
    shell: createTauriShellBridge(globalThis)
  })
  configureHttpClient(runtime.port, runtime.token)
  // 启动自检：适配层没装全时，症状是「页面某个功能悄悄不工作」，
  // 因此把关键全局的真实类型打出来（只有类型，没有任何内容）。
  //
  // 走日志器而不是 console.info：这一条是「渲染层真的起来了」的证据，落在日志
  // 文件里才能被售后/排查用上（打包版看不到控制台）。
  const globals = globalThis as unknown as Record<string, unknown>
  bootstrapLog.info(
    `适配层已装：dbAPI=${typeof globals.dbAPI} screenMonitorAPI=${typeof globals.screenMonitorAPI} api=${typeof globals.api}`
  )
}

/** 由 `services/axios-config` 注入，避免适配层反过来 import 业务模块。 */
let httpClientConfigurator: ((port: number, token: string) => void) | undefined

export function setHttpClientConfigurator(configure: (port: number, token: string) => void): void {
  httpClientConfigurator = configure
}

function configureHttpClient(port: number, token: string): void {
  httpClientConfigurator?.(port, token)
}

/** 便捷构造：连接 mc-daemon。 */
export function installHttpBackend(
  httpOptions: HttpBackendOptions,
  options: Omit<InstallOptions, 'backend'> = {}
): void {
  installAdapters({ ...options, backend: createHttpBackend(httpOptions) })
}
