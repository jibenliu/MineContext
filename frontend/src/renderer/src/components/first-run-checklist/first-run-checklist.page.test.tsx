import type { FirstRunStepView } from '@renderer/adapters/first-run-checklist'
import { fireEvent, render, screen } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import FirstRunChecklist from './index'

const steps = (current: FirstRunStepView['id']): FirstRunStepView[] => {
  const order: FirstRunStepView['id'][] = ['permission', 'apiKey', 'startRecording', 'firstScreenshot']
  const idx = order.indexOf(current)
  return order.map((id, i) => ({
    id,
    status: i < idx ? 'done' : i === idx ? 'current' : 'locked'
  }))
}

it('当前步为权限时展示开启权限 CTA', () => {
  const onRequestPermission = vi.fn()
  render(
    <FirstRunChecklist
      steps={steps('permission')}
      onRequestPermission={onRequestPermission}
      onGoApiKey={() => undefined}
      onStartRecording={() => undefined}
      onClearWaiting={() => undefined}
      onDismiss={() => undefined}
    />
  )
  expect(screen.getByTestId('first-run-step-permission')).toHaveAttribute('data-status', 'current')
  fireEvent.click(screen.getByTestId('first-run-cta-permission'))
  expect(onRequestPermission).toHaveBeenCalledTimes(1)
})

it('设置引导页 API Key 步只提示填表单，不跳转', () => {
  render(
    <FirstRunChecklist
      steps={steps('apiKey')}
      apiKeyInline
      onRequestPermission={() => undefined}
      onGoApiKey={() => undefined}
      onStartRecording={() => undefined}
      onClearWaiting={() => undefined}
      onDismiss={() => undefined}
    />
  )
  expect(screen.getByTestId('first-run-cta-api-key-hint')).toBeInTheDocument()
  expect(screen.queryByTestId('first-run-cta-api-key')).toBeNull()
})

it('等待首张截图时可清除等待态', () => {
  const onClearWaiting = vi.fn()
  render(
    <FirstRunChecklist
      steps={steps('firstScreenshot')}
      onRequestPermission={() => undefined}
      onGoApiKey={() => undefined}
      onStartRecording={() => undefined}
      onClearWaiting={onClearWaiting}
      onDismiss={() => undefined}
    />
  )
  expect(screen.getByTestId('first-run-waiting')).toBeInTheDocument()
  fireEvent.click(screen.getByTestId('first-run-cta-clear-waiting'))
  expect(onClearWaiting).toHaveBeenCalledTimes(1)
})

it('跳过引导会调用 onDismiss', () => {
  const onDismiss = vi.fn()
  render(
    <FirstRunChecklist
      steps={steps('permission')}
      onRequestPermission={() => undefined}
      onGoApiKey={() => undefined}
      onStartRecording={() => undefined}
      onClearWaiting={() => undefined}
      onDismiss={onDismiss}
    />
  )
  fireEvent.click(screen.getByTestId('first-run-dismiss'))
  expect(onDismiss).toHaveBeenCalledTimes(1)
})
