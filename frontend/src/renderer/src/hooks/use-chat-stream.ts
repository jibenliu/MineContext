// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import {
  ChatMessage,
  ChatStreamRequest,
  chatStreamService,
  StreamEvent,
  WorkflowStage
} from '@renderer/services/chat-stream-service'
import { getLogger } from '@shared/logger/renderer'
import { get } from 'lodash'
import { useCallback, useEffect, useRef, useState } from 'react'

const logger = getLogger('useChatStream')

export interface ChatState {
  messages: ChatMessage[]
  isLoading: boolean
  currentStage?: WorkflowStage
  progress: number
  sessionId: string
  messageId: number
  error?: string
}

export interface StreamingMessage {
  id: string
  role: 'assistant'
  content: string
  isStreaming: boolean
  stage?: WorkflowStage
  progress: number
  timestamp: string
}

export const useChatStream = () => {
  const [chatState, setChatState] = useState<ChatState>({
    messages: [],
    isLoading: false,
    progress: 0,
    sessionId: chatStreamService.generateSessionId(),
    messageId: -1
  })

  const [streamingMessage, setStreamingMessage] = useState<StreamingMessage | null>(null)
  const currentStreamingIdRef = useRef<string | null>(null)

  // Cleanup function
  useEffect(() => {
    return () => {
      chatStreamService.abortStream()
    }
  }, [])

  // Handle stream events
  const handleStreamEvent = useCallback((event: StreamEvent) => {
    // 只记事件类型，不把帧内容写进日志
    logger.debug('handling stream event:', event.type)

    switch (event.type) {
      case 'session_start':
        if (event.session_id) {
          setChatState((prev) => ({
            ...prev,
            sessionId: event.session_id!,
            messageId: get(event, 'assistant_message_id', prev.messageId)
          }))
        }
        break

      case 'thinking':
      case 'running':
        logger.debug('handling thinking event:', event.type)
        setChatState((prev) => ({
          ...prev,
          currentStage: event.stage,
          progress: event.progress
        }))

        // Update or create a streaming message to show the thinking process
        // but append instead of overwriting existing content
        setStreamingMessage((prev) => {
          if (prev && prev.stage === event.stage) {
            // If it's the same stage, update the content
            return {
              ...prev,
              content: event.content,
              progress: event.progress,
              timestamp: event.timestamp
            }
          } else {
            // Create a new thinking message
            return {
              id: 'thinking_' + Date.now(),
              role: 'assistant',
              content: event.content,
              isStreaming: true,
              stage: event.stage,
              progress: event.progress,
              timestamp: event.timestamp
            }
          }
        })
        break

      case 'stream_chunk':
        if (!currentStreamingIdRef.current) {
          currentStreamingIdRef.current = 'stream_' + Date.now()
          setStreamingMessage({
            id: currentStreamingIdRef.current,
            role: 'assistant',
            content: event.content,
            isStreaming: true,
            stage: event.stage,
            progress: event.progress,
            timestamp: event.timestamp
          })
        } else {
          setStreamingMessage((prev) => {
            if (prev) {
              return {
                ...prev,
                content: prev.content + event.content,
                progress: event.progress,
                timestamp: event.timestamp
              }
            }
            return null
          })
        }
        break

      case 'stream_complete':
        setStreamingMessage((prev) => {
          if (prev && prev.content.trim()) {
            const finalMessage: ChatMessage = {
              role: 'assistant',
              content: prev.content
            }

            setChatState((chatState) => ({
              ...chatState,
              messages: [...chatState.messages, finalMessage],
              isLoading: false,
              currentStage: 'completed',
              progress: 1.0
            }))

            currentStreamingIdRef.current = null
            return null
          }
          // No streaming message - just update state
          setChatState((chatState) => ({
            ...chatState,
            isLoading: false,
            currentStage: 'completed',
            progress: 1.0
          }))
          return null
        })
        break

      case 'completed':
        // Completed (non-streaming mode) - use content from event directly
        if (event.content && event.content.trim()) {
          const finalMessage: ChatMessage = {
            role: 'assistant',
            content: event.content
          }

          setChatState((prev) => ({
            ...prev,
            messages: [...prev.messages, finalMessage],
            isLoading: false,
            currentStage: 'completed',
            progress: 1.0
          }))
        } else {
          // Just update state if no content
          setChatState((prev) => ({
            ...prev,
            isLoading: false,
            currentStage: 'completed',
            progress: 1.0
          }))
        }
        setStreamingMessage(null)
        currentStreamingIdRef.current = null
        break

      case 'fail':
        setChatState((prev) => ({
          ...prev,
          error: event.content,
          isLoading: false,
          currentStage: 'failed'
        }))
        setStreamingMessage(null)
        break

      case 'done':
        setChatState((prev) => ({
          ...prev,
          currentStage: event.stage,
          progress: event.progress
        }))
        break
    }
  }, [])

  // Handle stream error
  const handleStreamError = useCallback((error: Error) => {
    logger.error('stream request error:', error)

    let errorMessage = error.message
    if (error.name === 'TypeError' && error.message.includes('fetch')) {
      errorMessage = 'Unable to connect to AI service, please check network connection and service status'
    } else if (error.name === 'AbortError') {
      errorMessage = 'Request has been cancelled'
    }

    setChatState((prev) => ({
      ...prev,
      error: errorMessage,
      isLoading: false
    }))
    setStreamingMessage(null)
    currentStreamingIdRef.current = null
  }, [])

  // Handle stream completion
  const handleStreamComplete = useCallback(() => {
    logger.debug('stream completed')
    setChatState((prev) => ({
      ...prev,
      isLoading: false
    }))
  }, [])

  // Send message
  const sendMessage = useCallback(
    async (query: string, conversation_id: number, context?: ChatStreamRequest['context']) => {
      if (!query.trim() || chatState.isLoading) return

      // Add user message
      const userMessage: ChatMessage = {
        role: 'user',
        content: query.trim()
      }

      setChatState((prev) => ({
        ...prev,
        messages: [...prev.messages, userMessage],
        isLoading: true,
        error: undefined
      }))

      // Clear previous streaming message
      setStreamingMessage(null)
      currentStreamingIdRef.current = null

      const request: ChatStreamRequest = {
        query: query.trim(),
        conversation_id,
        context: {
          ...context,
          chat_history: [...chatState.messages, userMessage]
        },
        session_id: chatState.sessionId
      }

      try {
        await chatStreamService.sendStreamMessage(request, handleStreamEvent, handleStreamError, handleStreamComplete)
      } catch (error) {
        handleStreamError(error as Error)
      }
    },
    [
      chatState.messages,
      chatState.isLoading,
      chatState.sessionId,
      handleStreamEvent,
      handleStreamError,
      handleStreamComplete
    ]
  )

  // Clear chat history
  const clearChat = useCallback(() => {
    chatStreamService.abortStream()
    setChatState({
      messages: [],
      isLoading: false,
      progress: 0,
      sessionId: chatStreamService.generateSessionId(),
      messageId: -1
    })
    setStreamingMessage(null)
    currentStreamingIdRef.current = null
  }, [])

  // Stop the current streaming request
  const stopStreaming = useCallback(() => {
    chatStreamService.abortStream()
    setChatState((prev) => ({
      ...prev,
      isLoading: false
    }))
    setStreamingMessage(null)
  }, [])

  return {
    ...chatState,
    streamingMessage,
    sendMessage,
    clearChat,
    stopStreaming,
    setChatState
  }
}
