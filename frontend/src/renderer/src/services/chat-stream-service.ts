// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { getLogger } from '@shared/logger/renderer'

import axiosInstance from './axios-config'

const logger = getLogger('ChatStreamService')

// Type definitions for the Chat Stream API
export interface ChatMessage {
  role: 'user' | 'assistant' | 'system'
  content: string
}

export interface DocumentInfo {
  id?: string
  title?: string
  content?: string
  summary?: string
  tags?: string[]
}

export interface ContextItem {
  source:
    | 'document'
    | 'web_search'
    | 'agent_memory'
    | 'context_db'
    | 'chat_history'
    | 'processed'
    | 'entity'
    | 'unknown'
  content: string
  title?: string
  relevance_score: number
  timestamp: string
  metadata?: Record<string, any>
}

export interface ChatContext {
  chat_history?: ChatMessage[]
  selected_content?: string
  document_id?: string
  current_document?: DocumentInfo
  collected_contexts?: ContextItem[]
  [key: string]: any // Other custom fields
}

export interface ChatStreamRequest {
  query: string
  context?: ChatContext
  session_id?: string
  user_id?: string
  conversation_id?: number
  page_name?: string
  vault_id?: number | null
}

export type EventType =
  | 'session_start'
  | 'thinking'
  | 'running'
  | 'done'
  | 'fail'
  | 'completed'
  | 'stream_chunk'
  | 'stream_complete'
  | 'error'
  | 'interrupted'

export type WorkflowStage =
  | 'init'
  | 'intent_analysis'
  | 'context_gathering'
  | 'execution'
  | 'reflection'
  | 'completed'
  | 'failed'
  | 'next'

export type NodeType = 'intent' | 'context' | 'execute' | 'reflect'

export interface StreamEvent {
  type: EventType
  content: string
  stage?: WorkflowStage
  node?: NodeType
  progress: number
  timestamp: string
  session_id?: string
  metadata?: {
    [key: string]: any
  }
  assistant_message_id?: number
  conversation_id?: number | null
  message?: string
  citations?: Array<{ document_id: string; title: string; kind: string }>
  /** `local` = 仅本地列表；`model` = 在线生成 */
  mode?: string
  model?: string | null
}

// Streaming chat service class
export class ChatStreamService {
  private abortController?: AbortController

  // Send a streaming chat request
  async sendStreamMessage(
    request: ChatStreamRequest,
    onEvent: (event: StreamEvent) => void,
    onError?: (error: Error) => void,
    onComplete?: () => void
  ): Promise<void> {
    // Cancel the previous request
    this.abortStream()

    this.abortController = new AbortController()

    try {
      // 端口只能来自运行时注入（`runtime.json`），没有就不发请求：
      // 退回写死的端口会让「地址未就绪」伪装成「后端连不上」。
      const baseUrl = axiosInstance.defaults.baseURL
      if (!baseUrl) {
        throw new Error('后端地址尚未就绪（还没读到 daemon 的 runtime.json）')
      }
      // **token 必须一起带上**：daemon 除 `/api/health` 外全部要鉴权，
      // 少这个头就是 401 —— 而流式接口 401 的表现是「AI 助手一直转圈」，
      // 不会有任何可见错误。token 与 baseURL 同源：由适配层在安装时写入。
      const token = axiosInstance.defaults.headers.common['X-MC-Token'] as string | undefined
      const response = await fetch(`${baseUrl}/api/agent/chat/stream`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(token ? { 'X-MC-Token': token } : {})
        },
        body: JSON.stringify(request),
        signal: this.abortController.signal
      })

      if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`)
      }

      const reader = response.body?.getReader()
      if (!reader) {
        throw new Error('Response body is not readable')
      }

      const decoder = new TextDecoder()
      let buffer = ''

      while (true) {
        const { done, value } = await reader.read()

        if (done) {
          break
        }

        buffer += decoder.decode(value, { stream: true })

        // Handle multi-line data
        const lines = buffer.split('\n')
        buffer = lines.pop() || '' // Keep the potentially incomplete last line

        for (const line of lines) {
          if (line.trim() === '') continue

          // 只记「收到一行」，不记行内容：SSE 行里是对话正文（日志纪律：不写用户内容）
          logger.debug('received sse line')

          if (line.startsWith('data: ')) {
            try {
              const jsonStr = line.slice(6) // Remove 'data: ' prefix
              logger.debug('parsing sse data')
              const eventData = JSON.parse(jsonStr)
              logger.debug('parsed sse data')
              onEvent(eventData as StreamEvent)
            } catch (parseError) {
              logger.warn('Failed to parse SSE event')
            }
          } else {
            logger.debug('non-data sse line ignored')
          }
        }
      }

      onComplete?.()
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        logger.info('stream request aborted')
        return
      }

      logger.error('stream request failed:', error)
      onError?.(error as Error)
    }
  }

  // Cancel the current streaming request
  abortStream(): void {
    if (this.abortController) {
      this.abortController.abort()
      this.abortController = undefined
    }
  }

  // Generate a session ID
  generateSessionId(): string {
    return 'session_' + Date.now() + '_' + Math.random().toString(36).substr(2, 9)
  }
}

// Export a singleton instance
export const chatStreamService = new ChatStreamService()
