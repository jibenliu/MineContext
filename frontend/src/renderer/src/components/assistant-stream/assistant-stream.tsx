import { Button, Input, Modal } from '@arco-design/web-react'
import { useNavigation } from '@renderer/hooks/use-navigation'
import { useVaults } from '@renderer/hooks/use-vault'
import { useI18n } from '@renderer/i18n'
import MarkdownIt from 'markdown-it'
import { FC, useRef, useState } from 'react'

import { citationPath } from './citation-target'
import { conversationDisplayTitle } from './conversation-title'
import { openCitation } from './open-citation'
import { useAssistantConversation } from './use-assistant-conversation'

const markdown = new MarkdownIt({ html: false, linkify: false, breaks: true })
const answerClass =
  'min-h-[44px] text-[13px] leading-6 text-[var(--color-text-1)] [&_code]:rounded [&_code]:bg-[var(--color-bg-3)] [&_code]:px-1 [&_ol]:my-1 [&_ol]:pl-5 [&_p]:my-1 [&_pre]:overflow-x-auto [&_pre]:rounded-[8px] [&_pre]:bg-[var(--color-bg-1)] [&_pre]:p-3 [&_ul]:my-1 [&_ul]:pl-5'

export const AssistantStream: FC = () => {
  const { t } = useI18n()
  const { selectedVaultId, setSelectedVaultId, getVaultRoots } = useVaults()
  const vaultRoots = getVaultRoots()
  const chat = useAssistantConversation(selectedVaultId)
  const { navigateToVault, navigateToMainTab } = useNavigation()
  const [draft, setDraft] = useState('')
  const [copyFeedback, setCopyFeedback] = useState('')
  const endRef = useRef<HTMLDivElement>(null)
  const disabled = chat.busy || chat.loading
  const untitled = t('home.untitledConversation')
  // 提问已乐观写入 history 时，不再另开一块空 live 气泡（否则像「字消失了」）。
  const inFlight = chat.history.some((turn) => turn.state === 'thinking' || turn.state === 'streaming')
  const live = chat.history.length === 0 || (chat.busy && !inFlight)

  const send = () => {
    const query = draft.trim()
    if (!query || disabled) return
    setDraft('')
    chat.ask(query)
  }
  const copy = async (answer: string) => {
    try {
      await navigator.clipboard.writeText(answer)
      setCopyFeedback(t('assistant.copied'))
    } catch {
      setCopyFeedback(t('assistant.copyFailed'))
    }
  }
  const confirmDelete = (id: number) => {
    Modal.confirm({
      title: t('assistant.deleteConfirmTitle'),
      content: t('assistant.deleteConfirmBody'),
      okText: t('common.delete'),
      onOk: () => chat.deleteConversation(id)
    })
  }

  return (
    <div className="assistant-stream flex h-full w-full min-w-0 flex-1 gap-4 self-stretch">
      <aside className="flex w-[220px] flex-shrink-0 flex-col gap-2 overflow-hidden rounded-2xl border border-[var(--color-border-2)] bg-[var(--color-fill-1)] p-3">
        {vaultRoots.length > 0 && (
          <select
            aria-label={t('assistant.vaultLabel')}
            disabled={chat.busy}
            value={selectedVaultId ?? ''}
            onChange={(event) => {
              const next = event.target.value ? Number(event.target.value) : null
              setSelectedVaultId(next)
              setDraft('')
              setCopyFeedback('')
            }}
            className="h-8 rounded-[6px] border border-[var(--color-border-2)] bg-[var(--color-bg-2)] px-2 text-[13px] text-[var(--color-text-1)]">
            <option value="">{t('assistant.vaultAll')}</option>
            {vaultRoots.map((root) => (
              <option key={root.id} value={root.id}>
                {root.title || t('assistant.vaultUntitled')}
              </option>
            ))}
          </select>
        )}
        <Button
          type="primary"
          long
          disabled={chat.busy}
          onClick={() => {
            chat.newConversation()
            setDraft('')
            setCopyFeedback('')
          }}>
          {t('assistant.newConversation')}
        </Button>
        {chat.conversations.length > 0 && (
          <select
            aria-label={t('assistant.conversationLabel')}
            disabled={chat.busy}
            value={chat.activeId ?? ''}
            onChange={(event) =>
              event.target.value ? void chat.switchTo(Number(event.target.value)) : chat.newConversation()
            }
            className="h-8 rounded-[6px] border border-[var(--color-border-2)] bg-[var(--color-bg-2)] px-2 text-[13px] text-[var(--color-text-1)]">
            <option value="">{t('assistant.conversation.new')}</option>
            {chat.conversations.map((conversation) => (
              <option key={conversation.id} value={conversation.id}>
                {conversationDisplayTitle(conversation, untitled)}
              </option>
            ))}
          </select>
        )}
        <div className="flex flex-1 flex-col gap-1 overflow-y-auto">
          {chat.conversations.length === 0 ? (
            <p className="px-1 py-2 text-xs text-[var(--color-text-3)]">{t('assistant.noConversations')}</p>
          ) : (
            chat.conversations.map((conversation) => {
              const title = conversationDisplayTitle(conversation, untitled)
              return (
                <div
                  key={conversation.id}
                  className="group flex items-stretch gap-1 rounded-[8px] hover:bg-[var(--color-bg-1)] aria-[current=true]:bg-[var(--color-primary-light-1)]"
                  aria-current={conversation.id === chat.activeId ? 'true' : undefined}>
                  <button
                    type="button"
                    disabled={chat.busy}
                    onClick={() => void chat.switchTo(conversation.id)}
                    className="min-w-0 flex-1 truncate px-3 py-2 text-left text-[13px] text-[var(--color-text-2)]">
                    {title}
                  </button>
                  <button
                    type="button"
                    disabled={chat.busy}
                    aria-label={`${t('common.delete')} ${title}`}
                    onClick={() => confirmDelete(conversation.id)}
                    className="flex-shrink-0 px-2 text-[12px] text-[var(--color-text-3)] opacity-0 hover:text-[rgb(var(--danger-6))] group-hover:opacity-100 focus:opacity-100">
                    {t('common.delete')}
                  </button>
                </div>
              )
            })
          )}
        </div>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        {chat.error && (
          <div role="alert">
            {chat.error}
            <Button
              onClick={() =>
                chat.activeId !== null ? void chat.switchTo(chat.activeId) : void chat.loadConversations()
              }
              disabled={chat.busy}>
              {t('common.retry')}
            </Button>
          </div>
        )}
        <div className="flex flex-1 flex-col gap-3 overflow-y-auto rounded-2xl border border-[var(--color-border-2)] bg-[var(--color-bg-2)] p-4">
          {chat.history.length > 0 && (
            <ul data-testid="assistant-history" className="flex flex-col gap-3">
              {chat.history.map((turn, index) => (
                <li key={turn.id} className="flex flex-col gap-2">
                  <div className="max-w-[80%] self-end rounded-2xl rounded-br-[4px] bg-[var(--color-primary-light-1)] px-4 py-2 text-[13px] text-[var(--color-text-1)]">
                    {turn.query}
                  </div>
                  <div className="max-w-[80%] self-start rounded-2xl rounded-bl-[4px] border border-[var(--color-border-2)] bg-[var(--color-bg-2)] px-4 py-2">
                    {turn.mode === 'local' && (
                      <span
                        data-testid="assistant-mode-local"
                        title={t('assistant.mode.localHint')}
                        className="mb-2 inline-block rounded-[4px] bg-[var(--color-fill-2)] px-1.5 py-0.5 text-[11px] text-[var(--color-text-3)]">
                        {t('assistant.mode.local')}
                      </span>
                    )}
                    <div
                      data-testid={!live && index === chat.history.length - 1 ? 'assistant-text' : undefined}
                      className={answerClass}
                      dangerouslySetInnerHTML={{ __html: markdown.render(turn.error ?? turn.answer) }}
                    />
                    {turn.sources.length > 0 && (
                      <details className="mt-2 text-xs text-[var(--color-text-3)]">
                        <summary>{t('assistant.sources', { count: turn.sources.length })}</summary>
                        <ul className="mt-1 flex flex-col gap-1">
                          {turn.sources.map((source, sourceIndex) => {
                            const href = citationPath(source)
                            const label = `${source.title} · ${source.kind}`
                            return (
                              <li key={source.kind + source.document_id + sourceIndex}>
                                {href ? (
                                  <button
                                    type="button"
                                    data-testid="assistant-citation"
                                    data-citation-kind={source.kind}
                                    data-citation-id={source.document_id}
                                    className="text-left text-[var(--color-primary)] underline-offset-2 hover:underline"
                                    onClick={() =>
                                      openCitation(source, { navigateToVault, navigateToMainTab })
                                    }>
                                    {label}
                                  </button>
                                ) : (
                                  <span data-testid="assistant-citation-static">{label}</span>
                                )}
                              </li>
                            )
                          })}
                        </ul>
                      </details>
                    )}
                    <div className="mt-2 flex gap-2">
                      {turn.answer && (
                        <Button size="mini" onClick={() => void copy(turn.answer)}>
                          {t('assistant.copy')}
                        </Button>
                      )}
                      {index === chat.history.length - 1 && (
                        <Button size="mini" disabled={disabled} onClick={() => chat.ask(turn.query)}>
                          {t('assistant.retryAnswer')}
                        </Button>
                      )}
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          )}
          {live && (
            <div
              data-testid="assistant-text"
              className={answerClass}
              dangerouslySetInnerHTML={{ __html: markdown.render(chat.text) }}
            />
          )}
          <div ref={endRef} />
        </div>
        {chat.progress && (
          <details open className="text-xs text-[var(--color-text-3)]">
            <summary>{t('assistant.progress')}</summary>
            <p>{chat.progress}</p>
          </details>
        )}
        <div data-testid="assistant-state" className="min-h-[20px] text-xs text-[var(--color-text-3)]">
          {chat.loading
            ? t('assistant.loadingHistory')
            : chat.state === 'thinking'
              ? t('assistant.thinking')
              : chat.state === 'streaming'
                ? t('assistant.streaming')
                : chat.state === 'failed'
                  ? t('assistant.failed')
                  : chat.state === 'stopped'
                    ? t('assistant.stopped')
                    : ''}
        </div>
        {copyFeedback && <p role="status">{copyFeedback}</p>}
        {chat.history.length > 0 && (
          <Button size="mini" onClick={() => endRef.current?.scrollIntoView?.({ block: 'end', behavior: 'smooth' })}>
            {t('assistant.latest')}
          </Button>
        )}
        <div className="flex items-end gap-2 rounded-2xl border border-[var(--color-border-2)] bg-[var(--color-bg-2)] p-3">
          <Input.TextArea
            aria-label={t('assistant.inputPlaceholder')}
            placeholder={t('assistant.inputPlaceholder')}
            autoSize={{ minRows: 1, maxRows: 4 }}
            value={draft}
            onChange={setDraft}
            onPressEnter={(event) => {
              if (!event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault()
                send()
              }
            }}
          />
          <Button type="primary" onClick={send} disabled={!draft.trim() || disabled}>
            {t('assistant.send')}
          </Button>
          {chat.busy ? (
            <Button onClick={() => void chat.stop()}>{t('assistant.stop')}</Button>
          ) : (
            <Button
              type="outline"
              disabled={chat.loading}
              onClick={() => chat.ask(t('assistant.defaultQuery'))}
              className="mc-secondary-btn !bg-white !border-[var(--color-border-3)] !text-[var(--color-text-1)]">
              {t('assistant.ask')}
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
