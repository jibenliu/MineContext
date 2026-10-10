// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import './vault.css'

import { Card, Message, Spin } from '@arco-design/web-react'
import { blobImageUrls, persistBlobImages } from '@renderer/adapters/markdown-images'
import AIAssistant from '@renderer/components/ai-assistant'
import AIToggleButton from '@renderer/components/ai-toggle-button'
import MarkdownEditor from '@renderer/components/markdown-editor'
import { toEditorFileUrl } from '@renderer/components/markdown-editor/images'
import StatusBar from '@renderer/components/status-bar/status-bar'
import { useAllotment } from '@renderer/hooks/use-allotment'
import { useVaults } from '@renderer/hooks/use-vault'
import { useI18n } from '@renderer/i18n'
import { RootState, useAppDispatch } from '@renderer/store'
import { setActiveConversationId, toggleCreationAiAssistant } from '@renderer/store/chat-history'
import { removeMarkdownSymbols } from '@renderer/utils/vault'
import { useUnmount } from 'ahooks'
import { Allotment } from 'allotment'
import { debounce } from 'lodash'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useSelector } from 'react-redux'
import { Link, useSearchParams } from 'react-router-dom'

const VaultPage = () => {
  const { t } = useI18n()
  const [searchParams] = useSearchParams()
  const id = searchParams.get('id')
  const { findVaultById, saveVaultContent, saveVaultTitle, updateVault, loading, initVaults } = useVaults()
  const [ready, setReady] = useState(false)

  useEffect(() => {
    let cancelled = false
    void initVaults().finally(() => {
      if (!cancelled) setReady(true)
    })
    return () => {
      cancelled = true
    }
  }, [initVaults])

  const vault = id ? findVaultById(Number(id)) : null
  const content = vault?.content
  const title = vault?.title ? '## ' + vault.title : ''
  const isVisible = useSelector((state: RootState) => state.chatHistory.creation.aiAssistantVisible)
  const { controller, defaultSizes, leftMinSize, rightMinSize } = useAllotment(isVisible)
  const dispatch = useAppDispatch()
  const debouncedSave = useMemo(
    () =>
      debounce((value: string, type: 'content' | 'title' | 'summary' | 'tags') => {
        if (!id) return
        if (type === 'content') {
          saveVaultContent(Number(id), value)
        } else if (type === 'title') {
          saveVaultTitle(Number(id), removeMarkdownSymbols(value))
        } else {
          updateVault(Number(id), { [type]: value })
        }
      }, 300),
    [saveVaultContent, saveVaultTitle, id, updateVault]
  )

  // 离开页面前把未到期的 debounce 冲掉，否则 300ms 内的最后一次编辑会丢。
  useUnmount(() => {
    debouncedSave.flush()
    debouncedSave.cancel()
    dispatch(setActiveConversationId(null))
    dispatch(toggleCreationAiAssistant(false))
  })

  // 切换笔记 id 时先冲掉旧 id 的挂起保存，避免写错目标。
  useEffect(() => {
    return () => {
      debouncedSave.flush()
      debouncedSave.cancel()
    }
  }, [debouncedSave, id])

  const onMarkdownChange = useCallback(
    (markdown: string, type: 'content' | 'title') => {
      // 编辑器粘贴/拖入的图片给的是 `blob:` 地址，只在当前会话有效 ——
      // 直接落库，重启后笔记里就是一堆破图（用户完全不知道为什么）。
      // 因此先交给本地文件服务换成持久路径；**任何一张失败就整体不保存**
      // （半替换的结果是「有的图在库里、有的图是临时地址」，更难排查），
      // 编辑器里还留着原文，用户可以重试。
      if (blobImageUrls(markdown).length === 0) {
        debouncedSave(markdown, type)
        return
      }
      void persistBlobImages(markdown, {
        read: async (url) => new Uint8Array(await (await fetch(url)).arrayBuffer()),
        save: async (name, data) => {
          const result = (await window.fileService?.saveFile(name, data)) as
            { success?: boolean; filePath?: string } | undefined
          if (!result?.success || !result.filePath) throw new Error('图片保存失败')
          // 与 Crepe `persistEditorImage` 同一契约：markdown 写 `file://`，重启才能读回。
          return toEditorFileUrl(result.filePath)
        }
      })
        .then((outcome) => {
          if (outcome.failed > 0) {
            Message.warning(`有 ${outcome.failed} 张图片未能保存到本地，本次改动未落库`)
            return
          }
          debouncedSave(outcome.markdown, type)
        })
        .catch(() => Message.warning('图片保存失败，本次改动未落库'))
    },
    [debouncedSave]
  )

  const onSummaryChange = useCallback(
    (summary: string) => {
      debouncedSave(summary, 'summary')
    },
    [debouncedSave]
  )

  const onTagsChange = useCallback(
    (tags: string) => {
      debouncedSave(tags, 'tags')
    },
    [debouncedSave]
  )
  const activeConversationId = useSelector((state: RootState) => state.chatHistory.activeConversationId)

  const showLoading = !ready || loading
  const missingId = !id
  const notFound = ready && !loading && !!id && !vault

  // Status bar component
  return (
    <div className={`flex flex-row h-full allotmentContainer ${!isVisible ? 'allotment-disabled' : ''}`}>
      <Allotment separator={false} ref={controller} defaultSizes={defaultSizes}>
        <Allotment.Pane minSize={leftMinSize}>
          <div style={{ height: '8px', appRegion: 'drag' } as React.CSSProperties} />
          <div className="vault-page-container">
            <Card className="vault-card">
              {showLoading ? (
                <div className="flex justify-center items-center h-full text-[var(--color-text-1)]">
                  <Spin tip={t('common.loading')} />
                </div>
              ) : missingId || notFound ? (
                <div className="flex flex-col justify-center items-center h-full gap-3 text-[var(--color-text-2)]">
                  <div>{t('vault.notFound')}</div>
                  <Link to="/" className="text-[rgb(var(--primary-6))]">
                    {t('vault.backHome')}
                  </Link>
                </div>
              ) : (
                <>
                  <div className="vault-title-container">
                    <MarkdownEditor
                      key={`title-${vault?.id}`}
                      defaultValue={title ?? ''}
                      onChange={(markdown) => onMarkdownChange(markdown, 'title')}
                    />
                  </div>
                  <StatusBar vaultData={vault} onSummaryChange={onSummaryChange} onTagsChange={onTagsChange} />
                  <MarkdownEditor
                    key={vault?.id}
                    defaultValue={content ?? ''}
                    onChange={(markdown) => onMarkdownChange(markdown, 'content')}
                  />
                </>
              )}
            </Card>
            <AIToggleButton onClick={() => dispatch(toggleCreationAiAssistant(true))} isActive={isVisible} />
          </div>
        </Allotment.Pane>
        <Allotment.Pane minSize={rightMinSize}>
          <AIAssistant
            visible={isVisible}
            onClose={() => dispatch(toggleCreationAiAssistant(false))}
            pageName="creation"
            initConversationId={activeConversationId}
          />
        </Allotment.Pane>
      </Allotment>
    </div>
  )
}

export default VaultPage
