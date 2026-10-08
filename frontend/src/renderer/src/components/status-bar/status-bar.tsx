// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Input, Tag } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { Vault } from '@renderer/types'
import dayjs from 'dayjs'
import React, { useState } from 'react'

interface StatusBarProps {
  vaultData: Vault | null
  onSummaryChange: (summary: string) => void
  onTagsChange: (tags: string) => void
}

const StatusBar: React.FC<StatusBarProps> = ({ vaultData, onSummaryChange, onTagsChange }) => {
  const [isEditingSummary, setIsEditingSummary] = useState(false)
  const [isEditingTags, setIsEditingTags] = useState(false)
  const { t } = useI18n()

  if (!vaultData) {
    return null
  }

  const renderRow = (label: string, value: React.ReactNode, className?: string): React.ReactElement => (
    <div className={`flex items-center !mb-[12px] !text-[14px] ${className || ''}`}>
      <span className="text-[var(--color-text-3)] w-[140px]">{label} :</span>
      <div className="text-[var(--color-text-1)] w-full">{value}</div>
    </div>
  )

  const tags =
    typeof vaultData.tags === 'string' && vaultData.tags.trim()
      ? vaultData.tags.split(',').map((tag) => tag.trim())
      : []

  return (
    <div className="status-bar-container">
      {renderRow(
        t('vault.status.createdAt'),
        vaultData.created_at
          ? dayjs(vaultData.created_at).format('MMMM D, YYYY HH:mm:ss')
          : t('vault.status.notAvailable')
      )}
      {renderRow(t('vault.status.captureMethod'), t('vault.status.methodCreation'))}
      {renderRow(t('vault.status.processMethod'), t('vault.status.methodSmartSummary'))}
      {renderRow(t('vault.status.contextType'), t('vault.status.contextKnowledge'))}
      {renderRow(
        t('vault.status.summary'),
        isEditingSummary ? (
          <Input.TextArea
            defaultValue={vaultData.summary || ''}
            onChange={onSummaryChange}
            onBlur={() => setIsEditingSummary(false)}
            placeholder={t('vault.status.summaryPlaceholder')}
            autoSize
            autoFocus
            className="w-full"
          />
        ) : (
          <div
            onClick={() => {
              setIsEditingSummary(true)
            }}
            className="min-h-[22px] cursor-text w-full">
            {vaultData.summary || <span className="text-[var(--color-text-4)]">{t('vault.status.noSummary')}</span>}
          </div>
        )
      )}
      {renderRow(
        t('vault.status.tags'),
        <div className="flex flex-wrap items-center min-h-[32px] gap-x-[4px] gap-y-[4px]">
          {tags.length > 0 ? (
            tags.map((tag, index) => (
              <Tag key={index} className="mr-[8px] mb-[4px]">
                {tag}
              </Tag>
            ))
          ) : (
            <div className="text-[var(--color-text-4)] mr-[8px]">{t('vault.status.noTags')}</div>
          )}
          {isEditingTags ? (
            <Input
              defaultValue=""
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  const value = e.currentTarget.value.trim()
                  if (value.includes(',')) {
                    return
                  }
                  if (value) {
                    const newTags = [...tags, value]
                    onTagsChange(newTags.join(', '))
                    setIsEditingTags(false)
                  }
                }
              }}
              onBlur={() => setIsEditingTags(false)}
              placeholder={t('vault.status.tagPlaceholder')}
              autoFocus
              className="!w-[120px] mr-[8px] mb-[4px] !inline-block"
            />
          ) : (
            <Tag
              className="mb-[4px] cursor-pointer bg-[var(--color-fill-2)] !border-[var(--color-border-2)] !border-dashed border"
              onClick={(e) => {
                e.stopPropagation()
                setIsEditingTags(true)
              }}>
              {t('vault.status.newTag')}
            </Tag>
          )}
        </div>,
        'items-start'
      )}
    </div>
  )
}

export default StatusBar
