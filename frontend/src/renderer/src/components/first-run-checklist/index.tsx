// 首次引导：轻量清单（非模态栈）。步骤门闩见 adapters/first-run-checklist。

import { Button, Typography } from '@arco-design/web-react'
import { type FirstRunStepId, type FirstRunStepView } from '@renderer/adapters/first-run-checklist'
import { useI18n } from '@renderer/i18n'
import React from 'react'

const { Text, Title } = Typography

export interface FirstRunChecklistProps {
  steps: FirstRunStepView[]
  onRequestPermission: () => void
  onGoApiKey: () => void
  onStartRecording: () => void
  onClearWaiting: () => void
  onDismiss: () => void
  /** 已在设置引导页时，API Key 步只提示填下方表单。 */
  apiKeyInline?: boolean
}

const STEP_TITLE_KEY: Record<FirstRunStepId, string> = {
  permission: 'onboarding.step.permission',
  apiKey: 'onboarding.step.apiKey',
  startRecording: 'onboarding.step.startRecording',
  firstScreenshot: 'onboarding.step.firstScreenshot'
}

const FirstRunChecklist: React.FC<FirstRunChecklistProps> = ({
  steps,
  onRequestPermission,
  onGoApiKey,
  onStartRecording,
  onClearWaiting,
  onDismiss,
  apiKeyInline = false
}) => {
  const { t } = useI18n()
  const current = steps.find((s) => s.status === 'current')?.id

  return (
    <div
      data-testid="first-run-checklist"
      className="mb-4 rounded-[12px] border border-[var(--color-border-2)] bg-[var(--color-bg-1)] px-4 py-3">
      <div className="mb-2 flex items-start justify-between gap-3">
        <div>
          <Title heading={6} style={{ margin: 0 }}>
            {t('onboarding.title')}
          </Title>
          <Text type="secondary" className="text-[12px]">
            {t('onboarding.subtitle')}
          </Text>
        </div>
        <Button type="text" size="mini" data-testid="first-run-dismiss" onClick={onDismiss}>
          {t('onboarding.dismiss')}
        </Button>
      </div>
      <ol className="m-0 flex list-none flex-col gap-2 p-0">
        {steps.map((step, index) => (
          <li
            key={step.id}
            data-testid={`first-run-step-${step.id}`}
            data-status={step.status}
            className="flex items-center gap-2 text-[13px] text-[var(--color-text-2)]">
            <span
              aria-hidden
              className={
                step.status === 'done'
                  ? 'inline-flex h-5 w-5 items-center justify-center rounded-full bg-[rgb(var(--success-6))] text-[11px] text-white'
                  : step.status === 'current'
                    ? 'inline-flex h-5 w-5 items-center justify-center rounded-full bg-[rgb(var(--primary-6))] text-[11px] text-white'
                    : 'inline-flex h-5 w-5 items-center justify-center rounded-full border border-[var(--color-border-3)] text-[11px] text-[var(--color-text-3)]'
              }>
              {step.status === 'done' ? '✓' : index + 1}
            </span>
            <span
              className={
                step.status === 'locked'
                  ? 'text-[var(--color-text-4)]'
                  : step.status === 'current'
                    ? 'font-medium text-[var(--color-text-1)]'
                    : undefined
              }>
              {t(STEP_TITLE_KEY[step.id])}
            </span>
          </li>
        ))}
      </ol>
      <div className="mt-3 flex flex-wrap items-center gap-2">
        {current === 'permission' ? (
          <Button
            type="primary"
            size="small"
            data-testid="first-run-cta-permission"
            onClick={onRequestPermission}
            className="!bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
            {t('onboarding.cta.permission')}
          </Button>
        ) : null}
        {current === 'apiKey' ? (
          apiKeyInline ? (
            <Text type="secondary" className="text-[12px]" data-testid="first-run-cta-api-key-hint">
              {t('onboarding.cta.apiKeyInline')}
            </Text>
          ) : (
            <Button
              type="primary"
              size="small"
              data-testid="first-run-cta-api-key"
              onClick={onGoApiKey}
              className="!bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
              {t('onboarding.cta.apiKey')}
            </Button>
          )
        ) : null}
        {current === 'startRecording' ? (
          <Button
            type="primary"
            size="small"
            data-testid="first-run-cta-start"
            onClick={onStartRecording}
            className="!bg-[rgb(var(--primary-6))] !border-[rgb(var(--primary-6))]">
            {t('onboarding.cta.startRecording')}
          </Button>
        ) : null}
        {current === 'firstScreenshot' ? (
          <>
            <Text type="secondary" className="text-[12px]" data-testid="first-run-waiting">
              {t('onboarding.waitingScreenshot')}
            </Text>
            <Button type="text" size="small" data-testid="first-run-cta-clear-waiting" onClick={onClearWaiting}>
              {t('onboarding.cta.clearWaiting')}
            </Button>
          </>
        ) : null}
      </div>
    </div>
  )
}

export default FirstRunChecklist
