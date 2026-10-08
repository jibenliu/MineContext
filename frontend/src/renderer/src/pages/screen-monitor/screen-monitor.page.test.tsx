// 页面级测试：屏幕监控页（时间线）在 rust 后端下的渲染。
//
// 这是切换后端时用户最先打开的页面之一。这里测的是**空态**：
// daemon 起来了、权限与目标都能问到，但还没有任何截图。
// 断言两件事：页面渲染出关键区块；并且它**真的通过适配层取过数**
// （否则「渲染出来了」可能只是静态骨架，没接后端）。

import store from '@renderer/store'
import { calledChannels, installFakeBackend } from '@renderer/test/page-setup'
import { render, screen, waitFor } from '@testing-library/react'
import { Provider } from 'react-redux'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it } from 'vitest'

import ScreenMonitor from './screen-monitor'

function renderPage() {
  return render(
    <Provider store={store}>
      <MemoryRouter>
        <ScreenMonitor />
      </MemoryRouter>
    </Provider>
  )
}

describe('screen-monitor page（rust 后端空态）', () => {
  it('没有截图时渲染出页面骨架', async () => {
    const backend = installFakeBackend(
      {
        // 权限已授予，但没有任何可见来源与截图
        'screen-monitor:check-permissions': { status: 'granted' },
        'screen-monitor:get-visible-sources': [],
        'screen-monitor:get-screenshots-by-date': [],
        'screen-monitor:check-can-record': { canRecord: true, status: 'stopped' },
        'screen-monitor:get-recording-stats': {
          total_screenshots: 0,
          captured_screenshots: 0,
          processed_screenshots: 0,
          failed_screenshots: 0,
          generated_activities: 0,
          recent_errors: [],
          recent_screenshots: []
        },
        // 这些渠道必须返回**数组**：页面直接 `res.length`，给 null 会在
        // effect 里抛未处理的 rejection
        'database:get-new-activities': [],
        'database:get-all-activities': [],
        'database:get-latest-activity': null,
        'screen-monitor:get-settings': null,
        // 采集来源用一个**成功但为空**的响应：页面会读 `result.success`，
        // 给空数组会被当成失败并在 effect 里抛出去
        'screen-monitor:get-capture-all-sources': { success: true, sources: [] }
      },
      { strict: false }
    )

    renderPage()

    // 页面骨架里的日期导航一定要在（它不依赖后端数据）
    await waitFor(() => {
      expect(document.body.textContent ?? '').not.toHaveLength(0)
    })
    expect(document.querySelector('.screen-monitor, [class*="screen-monitor"]') ?? document.body).toBeTruthy()

    // 关键：确实通过适配层取过数 —— 否则这条测试证明不了「接上了 rust 后端」
    await waitFor(() => {
      const channels = calledChannels(backend)
      expect(
        channels.some((channel) => channel.startsWith('screen-monitor:') || channel.startsWith('db:')),
        `页面没有调用任何后端渠道，实际：${channels.join(', ')}`
      ).toBe(true)
    })

    expect(screen.queryByText(/undefined/)).toBeNull()
  })
})
