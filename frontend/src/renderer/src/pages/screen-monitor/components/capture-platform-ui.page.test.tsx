import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import EmptyStatePlaceholder from './empty-state-placeholder'
import ScreenMonitorHeader from './screen-monitor-header'

describe('屏幕监控：非支持平台 UI 门闩', () => {
  it('空态：captureSupported=false 时说明平台限制，不出现开启权限按钮', () => {
    render(<EmptyStatePlaceholder hasPermission={false} isToday captureSupported={false} onGrantPermission={vi.fn()} />)

    expect(screen.getByTestId('capture-unsupported-empty')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /开启权限|Enable Permission/i })).toBeNull()
    expect(screen.getByText(/macOS|屏幕采集|screen capture/i)).toBeInTheDocument()
  })

  it('空态：平台支持但无权限时仍显示开启权限', () => {
    render(<EmptyStatePlaceholder hasPermission={false} isToday captureSupported onGrantPermission={vi.fn()} />)

    expect(screen.queryByTestId('capture-unsupported-empty')).toBeNull()
    expect(screen.getByRole('button', { name: /开启权限|Enable Permission/i })).toBeInTheDocument()
  })

  it('顶栏：不支持时开始录制禁用，并显示平台横幅', () => {
    render(
      <ScreenMonitorHeader
        hasPermission={false}
        captureSupported={false}
        isMonitoring={false}
        isToday
        screenAllSources={[]}
        appAllSources={[]}
        onOpenSettings={vi.fn()}
        onStartMonitoring={vi.fn()}
        onStopMonitoring={vi.fn()}
        onRequestPermission={vi.fn()}
      />
    )

    expect(screen.getByTestId('capture-unsupported-banner')).toBeInTheDocument()
    const start = screen.getByRole('button', { name: /开始录制|Start Recording/i })
    expect(start).toBeDisabled()
  })
})
