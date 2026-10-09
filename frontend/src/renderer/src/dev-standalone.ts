// 纯浏览器开发桩：`pnpm dev` 且没有 Tauri 外壳时使用。
//
// 背景：渲染层靠外壳注入的 `window.mcRuntime`（daemon 端口 + token）装 HTTP 后端。
// 纯浏览器没有外壳、也没有 daemon，bootstrap 会判定后端不可用，界面停在后端状态页，
// 看不到任何页面，也就无法在浏览器里看样式。
//
// 这里装一个**基于接口契约的 mock 后端**：给三页（搜索 / 助手 / 总结）返回结构化的
// 假数据，让页面在纯前端也能显示出内容与样式；其余渠道返回空对象，页面走各自空态。
//
// **只在 dev 构建且无外壳时被调用**（见 `main.tsx`）；生产构建会把引入它的条件
// 折成 false，整块（含本模块）被摇出。

import { installAdapters } from './adapters/install'
import type { Backend } from './adapters/types'
import axiosInstance, { configureHttpClient } from './services/axios-config'

// 假数据的时间基准：固定值（不取 now），保证同一次会话内渲染稳定。
const MOCK_NOW = '2026-01-01T10:00:00Z'

/** 基于接口契约的最小 mock 后端。渠道名见 `adapters/channel-map.ts`。 */
function createMockBackend(): Backend {
  const invoke = async (channel: string): Promise<unknown> => {
    switch (channel) {
      case 'v1:summaries':
        return {
          summaries: [
            {
              id: 'mock-summary-1',
              title: '上午：写导入脚本',
              body_markdown: '把旧库的笔记与待办导进新版数据目录，跑通迁移校验。',
              quality: 'model',
              start: '2026-01-01T09:00:00Z',
              end: '2026-01-01T11:30:00Z',
              evidence_count: 7
            },
            {
              id: 'mock-summary-2',
              title: '下午：梳理前端样式',
              body_markdown: '统一卡片与间距，补齐搜索 / 助手 / 总结三页的空态。',
              quality: 'fallback',
              start: '2026-01-01T13:30:00Z',
              end: '2026-01-01T16:00:00Z',
              evidence_count: 3
            }
          ]
        }
      case 'v1:conversations':
        // 与后端契约一致：`{items, total}`（不是裸数组）
        return {
          items: [
            { id: 1, title: '我今天做了什么？', page_name: 'home', updated_at: MOCK_NOW },
            { id: 2, title: '总结一下上午的工作', page_name: 'summaries', updated_at: MOCK_NOW }
          ],
          total: 2
        }
      case 'v1:conversation-messages':
        return [
          { id: 1, role: 'user', content: '我今天做了什么？' },
          { id: 2, role: 'assistant', content: '上午在写导入脚本，下午梳理了前端样式。' }
        ]
      case 'v1:search':
        return {
          results: [
            {
              id: 'mock-hit-1',
              kind: 'activity',
              title: '写导入脚本',
              snippet: '把旧库的笔记与待办导进新版数据目录。',
              score: 0.92,
              at: 1767261000000
            },
            {
              id: 'mock-hit-2',
              kind: 'summary',
              title: '上午：写导入脚本',
              snippet: '跑通迁移校验，确认没有丢数据。',
              score: 0.81,
              at: 1767261000000
            }
          ]
        }
      case 'database:get-all-vaults':
      case 'database:get-vaults-by-document-type':
        // 笔记树给一个示例节点：空树会把第三方树库（react-arborist）的边界情况探出来，
        // 而真实环境侧边栏总是有节点。
        return [
          {
            id: 1,
            title: '示例笔记',
            summary: '',
            content: '',
            tags: '',
            parent_id: null,
            is_folder: 0,
            is_deleted: 0,
            document_type: 'vaults',
            sort_order: 0,
            created_at: MOCK_NOW,
            updated_at: MOCK_NOW
          }
        ]
      case 'database:insert-vault':
        // 真实后端返回自增 id（number）。返回数组会把节点 id 变成 `[]`，
        // 而树库要求 id 非空字符串（`!id` 会抛）。
        return 2
      case 'database:update-vault-by-id':
      case 'database:delete-vault-by-id':
      case 'database:soft-delete-vault-by-id':
      case 'database:restore-vault-by-id':
      case 'database:hard-delete-vault-by-id':
        return true
      case 'database:get-vault-by-id':
      case 'database:get-vault-by-title':
      case 'database:get-vaults-by-parent-id':
      case 'database:get-folders':
        return []
      case 'screen-monitor:check-permissions':
        // 与真后端 `/api/capture/permissions` 对齐，便于纯浏览器看有权限态
        return {
          screen_recording: true,
          permission: 'granted',
          ready: true,
          running: false,
          message: null
        }
      case 'task:check-can-record':
        return { canRecord: true, status: 'stopped', reason: null }
      case 'screen-monitor:get-capture-all-sources':
        return {
          success: true,
          sources: [
            {
              id: 'screen:0',
              name: 'Built-in Display',
              type: 'screen',
              // 1×1 PNG：设置页「选择录制内容」要能渲染缩略图而不是灰裂图
              thumbnail: 'data:image/png;base64,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',
              appIcon: null,
              isVisible: true,
              selected: true
            },
            {
              id: 'window-1',
              name: 'Visual Studio Code',
              type: 'window',
              thumbnail: null,
              appIcon: null,
              appName: 'Visual Studio Code',
              windowTitle: 'main.rs — VSCode',
              isVisible: true,
              selected: false
            }
          ]
        }
      default:
        // 多数没显式列出的渠道期待的是**数组**（vault 树、活动列表、待办等）。返回 `[]`
        // 而不是 `{}`：页面里的 .length / .map / isEmpty 都不会因拿到对象而读到
        // undefined（例如 vault-tree 的 `data.id.toString()`）。
        return []
    }
  }
  return {
    kind: 'http',
    // mock 只需按渠道返回结构化数据，不必逐个满足调用方的泛型参数。
    invoke: invoke as unknown as Backend['invoke'],
    subscribe: () => () => {}
  }
}

/**
 * 给 axios 装内存适配器。
 *
 * `services/*`（设置页保存/读取模型配置）走的是 axios 而不是适配层，纯浏览器里没 daemon
 * 时会打到默认端口并报「网络错误」——表现就是「填完密钥进不去」。这里按路径返回成功/空数据，
 * 让设置页能保存并进入内部页。
 */
function installAxiosMock(): void {
  // 纯浏览器：把保存过的模型配置留在内存里，设置页才能回显脱敏密钥与复制。
  const stored: {
    config: Record<string, string>
    hasApiKey: boolean
    apiKeyMasked: string
    apiKeyPlain: string
  } = {
    config: {
      modelPlatform: '',
      modelId: 'doubao-seed-1-6-flash-250828',
      baseUrl: 'https://ark.cn-beijing.volces.com/api/v3',
      apiKey: '',
      embeddingModelId: 'doubao-embedding-vision-250615',
      embeddingBaseUrl: 'https://ark.cn-beijing.volces.com/api/v3',
      embeddingApiKey: ''
    },
    hasApiKey: false,
    apiKeyMasked: '',
    apiKeyPlain: ''
  }

  axiosInstance.defaults.adapter = async (config) => {
    const url = config.url ?? ''
    let data: unknown = { code: 0, data: {} }
    if (url.includes('/api/model_settings/api_key')) {
      data = stored.apiKeyPlain
        ? { code: 0, data: { apiKey: stored.apiKeyPlain } }
        : { code: 1, message: '尚未保存 API Key', data: null }
    } else if (url.includes('/api/model_settings/get')) {
      data = {
        code: 0,
        data: {
          config: { ...stored.config, apiKey: '', embeddingApiKey: '' },
          hasApiKey: stored.hasApiKey,
          apiKeyMasked: stored.apiKeyMasked
        }
      }
    } else if (url.includes('/api/model_settings/update')) {
      const body = typeof config.data === 'string' ? JSON.parse(config.data) : config.data
      const next = (body?.config ?? {}) as Record<string, string>
      const key = String(next.apiKey || next.embeddingApiKey || '').trim()
      if (key) {
        stored.apiKeyPlain = key
        stored.hasApiKey = true
        stored.apiKeyMasked = key.length <= 8 ? '••••••••' : `${key.slice(0, 4)}••••••••${key.slice(-4)}`
      }
      stored.config = {
        ...stored.config,
        ...next,
        apiKey: '',
        embeddingApiKey: ''
      }
      data = { code: 0, data: { success: true, message: 'dev-standalone: saved' } }
    } else if (url.includes('/api/events/fetch')) {
      // 事件轮询：给空列表，页面按「没有新事件」处理
      data = { code: 0, data: { events: [] } }
    } else if (url.includes('/api/conversations/list')) {
      data = { code: 0, data: { items: [], total: 0, hasMore: false } }
    }
    return { data, status: 200, statusText: 'OK', headers: {}, config } as never
  }
}

/**
 * 纯浏览器开发：对话流走的是 `fetch`（SSE，需要边收边读），没有 daemon 时请求会直接失败。
 * 这里拦下 `chat/stream` 返回一段模拟 SSE，让“提问→出字→完成”的前端状态机能跑通。
 *
 * 注意：mock 没有后端，**不会真的落库**，因此侧栏列表不会新增（那是真后端的行为）。
 */
function installChatStreamMock(): void {
  const originalFetch = globalThis.fetch?.bind(globalThis)
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url
    if (url.includes('/api/agent/chat/stream')) {
      const encoder = new TextEncoder()
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          const send = (payload: unknown): void => {
            controller.enqueue(encoder.encode(`data: ${JSON.stringify(payload)}\n\n`))
          }
          send({
            type: 'session_start',
            session_id: 'dev-standalone',
            assistant_message_id: 1,
            conversation_id: 1
          })
          for (const chunk of ['（开发态示例）', '这是模拟的', '流式回答。']) {
            send({ type: 'stream_chunk', content: chunk })
          }
          send({ type: 'completed' })
          send({ type: 'done' })
          controller.close()
        }
      })
      return new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } })
    }
    if (!originalFetch) throw new Error(`dev-standalone: 未 mock 的请求 ${url}`)
    return originalFetch(input, init)
  }) as typeof fetch
}

export function installDevStandalone(): void {
  installAdapters({ backend: createMockBackend(), shellCapabilities: {} })
  // 本模块已经把 axios 与 fetch 都接管了（见下面两个 mock），这里只需要「地址存在」：
  // 业务层直接走 axios 的那几条路径（对话流、设置页）在 baseURL 为空时会判定
  // 「后端地址尚未就绪（还没读到 daemon 的 runtime.json）」—— 而纯浏览器环境里
  // 既没有 daemon 也没有 runtime.json，那句话是误导。
  configureHttpClient(0, 'dev-standalone')
  installAxiosMock()
  installChatStreamMock()
}
