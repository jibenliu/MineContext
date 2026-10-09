import { installFakeBackend } from '@renderer/test/page-setup'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactElement } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'

import RecordingStatsCard, { type RecordingStats } from './components/recording-stats-card'

function renderWithRouter(ui: ReactElement) {
  const view = render(<MemoryRouter>{ui}</MemoryRouter>)
  return {
    ...view,
    rerender: (next: ReactElement) => view.rerender(<MemoryRouter>{next}</MemoryRouter>)
  }
}

const stats: RecordingStats = {
  // 采到 7 张、其中 1 张分析出了结果 —— 两个数是两件事
  captured_screenshots: 7,
  processed_screenshots: 1,
  failed_screenshots: 0,
  generated_activities: 0,
  next_activity_eta_seconds: 0,
  recent_errors: [],
  recent_screenshots: ['20261008/截图 #1.png']
}

describe('录制统计文案', () => {
  it('采到的张数与分析出结果的张数分开显示', async () => {
    installFakeBackend({})
    renderWithRouter(<RecordingStatsCard stats={stats} />)

    expect(screen.getByText('7')).toBeInTheDocument()
    expect(screen.getByText(/张截图已采集/)).toBeInTheDocument()
    expect(screen.getByText('1')).toBeInTheDocument()
    expect(screen.getByText(/张已分析/)).toBeInTheDocument()
  })
})

describe('截图缩略图', () => {
  it('通过受鉴权的适配层加载数据，不使用 file URL，轮询不重复加载', async () => {
    const backend = installFakeBackend({
      'screen-monitor:read-image-base64': { data: 'aW1hZ2U=', mime: 'image/jpeg' }
    })
    const view = renderWithRouter(<RecordingStatsCard stats={stats} />)
    await waitFor(() =>
      expect(screen.getByAltText('screenshot-1')).toHaveAttribute('src', 'data:image/jpeg;base64,aW1hZ2U=')
    )
    expect(backend.calls[0].args).toEqual([stats.recent_screenshots[0]])
    view.rerender(<RecordingStatsCard stats={{ ...stats, recent_screenshots: [...stats.recent_screenshots] }} />)
    expect(backend.calls).toHaveLength(1)
  })

  it('读取失败时显示可重试状态，恢复后显示图片', async () => {
    installFakeBackend({})
    const read = vi.spyOn(window.screenMonitorAPI, 'readImageAsBase64')
    read.mockRejectedValueOnce(new Error('missing')).mockResolvedValue({ success: true, data: 'aW1hZ2U=' })
    renderWithRouter(<RecordingStatsCard stats={stats} />)
    fireEvent.click(await screen.findByRole('button', { name: /重试/ }))
    await waitFor(() =>
      expect(screen.getByAltText('screenshot-1')).toHaveAttribute('src', 'data:image/png;base64,aW1hZ2U=')
    )
    expect(read).toHaveBeenCalledTimes(2)
  })

  it('失效图片显示重试，不留下破图', async () => {
    installFakeBackend({ 'screen-monitor:read-image-base64': { data: 'invalid' } })
    renderWithRouter(<RecordingStatsCard stats={stats} />)
    const image = await screen.findByAltText('screenshot-1')
    fireEvent.error(image)
    expect(await screen.findByRole('button', { name: /重试/ })).toBeVisible()
    expect(screen.queryByAltText('screenshot-1')).toBeNull()
  })

  it('列表更新后忽略旧图片的迟到响应', async () => {
    installFakeBackend({})
    let finishOld!: (value: { success: boolean; data: string }) => void
    const pending = new Promise<{ success: boolean; data: string }>((resolve) => {
      finishOld = resolve
    })
    vi.spyOn(window.screenMonitorAPI, 'readImageAsBase64')
      .mockReturnValueOnce(pending)
      .mockResolvedValue({ success: true, data: 'new-image' })
    const view = renderWithRouter(<RecordingStatsCard stats={stats} />)
    view.rerender(<RecordingStatsCard stats={{ ...stats, recent_screenshots: ['new.png'] }} />)
    await waitFor(() =>
      expect(screen.getByAltText('screenshot-1')).toHaveAttribute('src', 'data:image/png;base64,new-image')
    )
    await act(async () => finishOld({ success: true, data: 'old-image' }))
    expect(screen.getByAltText('screenshot-1')).toHaveAttribute('src', 'data:image/png;base64,new-image')
  })
})
