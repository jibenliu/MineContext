import { useI18n } from '@renderer/i18n'
import { chatStreamService } from '@renderer/services/chat-stream-service'
import { messageService } from '@renderer/services/messages-service'
import { getLogger } from '@shared/logger/renderer'
import { useCallback, useEffect, useRef, useState } from 'react'

const logger = getLogger('assistant-conversation')
export type ConversationState = 'idle' | 'thinking' | 'streaming' | 'completed' | 'failed' | 'stopped'
export interface Source {
  document_id: string
  title: string
  kind: string
}
interface Conversation {
  id: number
  title?: string | null
}
interface StoredMessage {
  id?: number
  role?: string
  content?: string
  metadata?: string
  status?: string
}
export interface Turn {
  id: string
  query: string
  answer: string
  state: ConversationState
  sources: Source[]
  error?: string
}
interface ChatApi {
  listConversations?: (
    limit?: number,
    vaultId?: number | null
  ) => Promise<Conversation[] | { items?: Conversation[] }>
  listMessages?: (id: number) => Promise<StoredMessage[]>
  deleteConversation?: (id: number) => Promise<unknown>
}
const api = () => (window as unknown as { chatApi?: ChatApi }).chatApi

function sourcesFrom(value: unknown): Source[] {
  if (!Array.isArray(value)) return []
  return value.filter(
    (source): source is Source =>
      source &&
      typeof source.document_id === 'string' &&
      typeof source.title === 'string' &&
      typeof source.kind === 'string'
  )
}

function storedSources(metadata?: string): Source[] {
  try {
    return sourcesFrom(JSON.parse(metadata ?? '{}').sources)
  } catch {
    return []
  }
}

export function useAssistantConversation(vaultId: number | null = null) {
  const { t } = useI18n()
  const [state, setState] = useState<ConversationState>('idle')
  const [text, setText] = useState('')
  const [progress, setProgress] = useState('')
  const [error, setError] = useState('')
  const [history, setHistory] = useState<Turn[]>([])
  const [conversations, setConversations] = useState<Conversation[]>([])
  const [activeId, setActiveId] = useState<number | null>(null)
  const [loading, setLoading] = useState(false)
  const requestRef = useRef(0)
  const mountedRef = useRef(true)
  const loadingRef = useRef(false)
  const pendingRef = useRef<{ messageId?: number; finish: (state: ConversationState, error?: string) => void } | null>(
    null
  )
  const vaultIdRef = useRef(vaultId)
  vaultIdRef.current = vaultId

  const loadConversations = useCallback(async () => {
    try {
      const result = await api()?.listConversations?.(20, vaultId)
      const rows = Array.isArray(result) ? result : result?.items
      if (mountedRef.current) setConversations(Array.isArray(rows) ? rows : [])
    } catch {
      if (mountedRef.current) setError(t('assistant.historyFailed'))
    }
  }, [t, vaultId])

  const newConversation = useCallback(() => {
    if (pendingRef.current) return
    requestRef.current += 1
    loadingRef.current = false
    setLoading(false)
    setActiveId(null)
    setHistory([])
    setText('')
    setProgress('')
    setError('')
    setState('idle')
  }, [])

  useEffect(() => {
    mountedRef.current = true
    void loadConversations()
    return () => {
      mountedRef.current = false
      requestRef.current += 1
      const pending = pendingRef.current
      if (pending) {
        if (pending.messageId !== undefined)
          void messageService
            .interruptMessageGeneration(pending.messageId)
            .catch(() => logger.warn('Interrupt on unmount failed'))
        chatStreamService.abortStream()
        pendingRef.current = null
      }
    }
  }, [loadConversations])

  // Switching vault clears the active session so history cannot bleed across roots.
  const previousVaultRef = useRef<number | null | undefined>(undefined)
  useEffect(() => {
    if (previousVaultRef.current === undefined) {
      previousVaultRef.current = vaultId
      return
    }
    if (previousVaultRef.current === vaultId) return
    previousVaultRef.current = vaultId
    newConversation()
    void loadConversations()
  }, [vaultId, newConversation, loadConversations])

  const deleteConversation = async (id: number) => {
    if (pendingRef.current) return
    const remove = api()?.deleteConversation
    if (!remove) {
      if (mountedRef.current) setError(t('assistant.deleteFailed'))
      return
    }
    try {
      await remove(id)
      if (!mountedRef.current) return
      setConversations((previous) => previous.filter((conversation) => conversation.id !== id))
      if (activeId === id) newConversation()
    } catch {
      if (mountedRef.current) setError(t('assistant.deleteFailed'))
    }
  }

  const switchTo = async (id: number) => {
    if (pendingRef.current) return
    const request = ++requestRef.current
    loadingRef.current = true
    setLoading(true)
    setActiveId(id)
    setHistory([])
    setText('')
    setProgress('')
    setError('')
    setState('idle')
    try {
      const read = api()?.listMessages
      if (!read) throw new Error('Message history unavailable')
      const rows = await read(id)
      if (request !== requestRef.current || !mountedRef.current) return
      const turns: Turn[] = []
      for (const row of Array.isArray(rows) ? rows : []) {
        if (row.role === 'user')
          turns.push({
            id: `stored-${row.id ?? turns.length}`,
            query: row.content ?? '',
            answer: '',
            state: 'completed',
            sources: []
          })
        else if (row.role === 'assistant' && turns.length) {
          const turn = turns[turns.length - 1]
          turn.answer = row.content ?? ''
          turn.sources = storedSources(row.metadata)
        }
      }
      setHistory(turns)
      setText(turns.at(-1)?.answer ?? '')
    } catch {
      if (request === requestRef.current && mountedRef.current) setError(t('assistant.historyFailed'))
    } finally {
      if (request === requestRef.current && mountedRef.current) {
        loadingRef.current = false
        setLoading(false)
      }
    }
  }

  const ask = (value: string) => {
    const query = value.trim()
    if (!query || pendingRef.current || loadingRef.current) return
    const request = ++requestRef.current
    const turnId = `turn-${request}`
    let answer = ''
    let sources: Source[] = []
    let finished = false
    const current = () => mountedRef.current && request === requestRef.current && !finished
    const patchTurn = (patch: Partial<Turn>) => {
      setHistory((previous) => previous.map((turn) => (turn.id === turnId ? { ...turn, ...patch } : turn)))
    }
    const finish = (finalState: ConversationState, failure?: string) => {
      if (!current()) return
      finished = true
      pendingRef.current = null
      setState(finalState)
      setText(failure ?? answer)
      setProgress('')
      // 发送时已插入提问；这里只收尾，避免「输入被清空却要等流结束才看见自己的话」。
      patchTurn({ answer, sources, state: finalState, error: failure })
    }
    pendingRef.current = { finish }
    setText('')
    setProgress('')
    setError('')
    setState('thinking')
    setHistory((previous) => [...previous, { id: turnId, query, answer: '', sources: [], state: 'thinking' }])
    void chatStreamService
      .sendStreamMessage(
        {
          query,
          conversation_id: activeId ?? undefined,
          page_name: activeId === null ? 'assistant' : undefined,
          vault_id: activeId === null ? vaultIdRef.current : undefined
        },
        (event) => {
          if (!current()) return
          if (event.type === 'session_start') {
            if (typeof event.assistant_message_id === 'number' && pendingRef.current)
              pendingRef.current.messageId = event.assistant_message_id
            if (typeof event.conversation_id === 'number') {
              setActiveId(event.conversation_id)
              void loadConversations()
            }
          } else if (event.type === 'thinking' || event.type === 'running') {
            setProgress(event.content ?? '')
          } else if (event.type === 'stream_chunk') {
            answer += event.content ?? ''
            setText(answer)
            setState('streaming')
            patchTurn({ answer, state: 'streaming' })
          } else if (event.type === 'stream_complete') {
            if (typeof event.content === 'string') answer = event.content
            sources = sourcesFrom(event.citations)
            finish('completed')
          } else if (event.type === 'fail' || event.type === 'error')
            finish('failed', event.message ?? event.content ?? t('assistant.generationFailed'))
          else if (event.type === 'interrupted') finish('stopped')
          else if (event.type === 'completed') {
            if (typeof event.content === 'string' && event.content.trim()) answer = event.content
            finish('completed')
          } else if (event.type === 'done') finish('completed')
        },
        () => finish('failed', t('assistant.generationFailed')),
        () => finish('completed')
      )
      .catch(() => finish('failed', t('assistant.generationFailed')))
  }

  const stop = async () => {
    const pending = pendingRef.current
    if (!pending) return
    try {
      if (pending.messageId !== undefined) await messageService.interruptMessageGeneration(pending.messageId)
      if (pendingRef.current !== pending || !mountedRef.current) return
      pending.finish('stopped')
      chatStreamService.abortStream()
    } catch {
      if (pendingRef.current === pending && mountedRef.current) setError(t('assistant.stopFailed'))
    }
  }
  return {
    state,
    text,
    progress,
    error,
    history,
    conversations,
    activeId,
    loading,
    busy: state === 'thinking' || state === 'streaming',
    ask,
    stop,
    switchTo,
    newConversation,
    deleteConversation,
    loadConversations
  }
}
