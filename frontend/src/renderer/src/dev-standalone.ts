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
              thumbnail:
                'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAF4AAAA8CAIAAABgjJpoAAABV0lEQVR42u3aSw7CMAxFUSfpOlgTq2O9DPgMkNuGxnHk54cYVKLC6IgrPnW53R9FpL7vRT0o+w8NnKOfVg6foYqUsyln5/S+4E2qfG7F8cB53JW5Xxq6/B5s0uiiH2xS6bI+qEgunkEFc3ELKp6LT1AhXRyCiuoyO6jALlODiu0yL6jwLpOCQnCZERSIi3lQOC62QUG5GAaF5mIVFKCLSVCYLuNBwboMBoXsMhIUuMvloPBdrgWVwuX1rqGLPu6voBK5/BVULpf+oNK5dAaV0aUnqKQup0HldTkOKrXLQVDZXfaCooseFF30oOiiB0UXPSi66EHRRQ+KLiLrtrLiuawKKsY4/6DCjMPeG5axKwqNLouDipct5N6wGF2ia3RZE1Tgjz+kvWGxvubd6OIaFMLXSIC9YZm2RNLo4hEU1M+xuHvD06dYBQX4890kKMy/NcLtDfuNewI6hgpW96OlhAAAAABJRU5ErkJggg==',
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
  const maskKey = (key: string) => (key.length <= 8 ? '••••••••' : `${key.slice(0, 4)}••••••••${key.slice(-4)}`)

  const stored: {
    config: Record<string, string>
    hasApiKey: boolean
    apiKeyMasked: string
    apiKeyPlain: string
    hasEmbeddingApiKey: boolean
    embeddingApiKeyMasked: string
    embeddingApiKeyPlain: string
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
    apiKeyPlain: '',
    hasEmbeddingApiKey: false,
    embeddingApiKeyMasked: '',
    embeddingApiKeyPlain: ''
  }

  axiosInstance.defaults.adapter = async (config) => {
    const url = config.url ?? ''
    let data: unknown = { code: 0, data: {} }
    if (url.includes('/api/model_settings/api_key')) {
      const wantEmbed =
        String(config.params?.field || '').toLowerCase() === 'embedding' || url.includes('field=embedding')
      const plain = wantEmbed ? stored.embeddingApiKeyPlain || stored.apiKeyPlain : stored.apiKeyPlain
      data = plain ? { code: 0, data: { apiKey: plain } } : { code: 1, message: '尚未保存 API Key', data: null }
    } else if (url.includes('/api/model_settings/get')) {
      data = {
        code: 0,
        data: {
          config: { ...stored.config, apiKey: '', embeddingApiKey: '' },
          hasApiKey: stored.hasApiKey,
          apiKeyMasked: stored.apiKeyMasked,
          hasEmbeddingApiKey: stored.hasEmbeddingApiKey || stored.hasApiKey,
          embeddingApiKeyMasked: stored.embeddingApiKeyMasked || stored.apiKeyMasked
        }
      }
    } else if (url.includes('/api/model_settings/update')) {
      const body = typeof config.data === 'string' ? JSON.parse(config.data) : config.data
      const next = (body?.config ?? {}) as Record<string, string>
      const vision = String(next.apiKey || '').trim()
      const embed = String(next.embeddingApiKey || '').trim()
      if (vision) {
        stored.apiKeyPlain = vision
        stored.hasApiKey = true
        stored.apiKeyMasked = maskKey(vision)
      }
      if (embed) {
        stored.embeddingApiKeyPlain = embed
        stored.hasEmbeddingApiKey = true
        stored.embeddingApiKeyMasked = maskKey(embed)
      } else if (vision && !stored.embeddingApiKeyPlain) {
        // 标准平台：向量与视觉共用
        stored.embeddingApiKeyPlain = vision
        stored.hasEmbeddingApiKey = true
        stored.embeddingApiKeyMasked = stored.apiKeyMasked
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
