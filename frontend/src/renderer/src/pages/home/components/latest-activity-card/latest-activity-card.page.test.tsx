// 页面级测试：首页「最新活动」卡片（真实渲染层 + 适配层 + 假后端）。
//
// 这一条补的是规划里一直挂着的 SKIP：「适配层逻辑已测，但没在真实渲染层跑过」。
// 它验证的是**整条链路**：
//
//   window.serverPushAPI（适配层） → 组件订阅 → 渲染出后端给的数据
//
// 组件的取数方式是「适配层把 `serverPushAPI` 装到 `window` 上，组件直接调用」，
// 因此这里不需要真的起 daemon：装一个假后端，手动推一条事件即可。

import store from '@renderer/store'
import { installFakeBackend } from '@renderer/test/page-setup'
import { act, render, screen, waitFor } from '@testing-library/react'
import { Provider } from 'react-redux'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it } from 'vitest'

import { LatestActivityCard } from './index'

function renderCard() {
  return render(
    <Provider store={store}>
      <MemoryRouter>
        <LatestActivityCard title="Latest activity" />
      </MemoryRouter>
    </Provider>
  )
}

const activity = {
  id: 1,
  title: '季度财报整理',
  content: 'Excel + 浏览器',
  resources: '[]',
  start_time: '2026-09-30 09:00:00',
  end_time: '2026-09-30 09:30:00'
}

describe('home/latest-activity-card', () => {
  it('没有推送时显示空态', async () => {
    installFakeBackend({ 'home:get-latest-activity': { success: true } })

    renderCard()

    expect(await screen.findByText('最近 7 天没有活动')).toBeInTheDocument()
  })

  it('推送一条活动后渲染出它的标题', async () => {
    const backend = installFakeBackend({ 'home:get-latest-activity': { success: true } })

    renderCard()

    // 组件挂载时会先发 'running'（打开推送开关）
    await waitFor(() => {
      expect(backend.calls.map((call) => call.channel)).toContain('home:get-latest-activity')
    })
    expect(backend.calls[0].args[0]).toBe('running')

    // 推送会触发 React 状态更新，包在 act 里避免「未包裹的更新」警告
    act(() => backend.push('push:latest-activity', activity))

    expect(await screen.findByText('季度财报整理')).toBeInTheDocument()
  })

  it('卸载时把推送开关关掉', async () => {
    const backend = installFakeBackend({ 'home:get-latest-activity': { success: true } })

    const view = renderCard()
    await waitFor(() => expect(backend.calls.length).toBeGreaterThan(0))

    view.unmount()

    // 「stopped 必须真的停」：否则一次挂载就把轮询永久留在进程里
    await waitFor(() => {
      const stops = backend.calls.filter(
        (call) => call.channel === 'home:get-latest-activity' && call.args[0] === 'stopped'
      )
      expect(stops.length).toBe(1)
    })
  })
})

// 同一条链路，但后端换成**真的 HttpBackend**（假 fetch + 假 SSE）：
// 这一条对应「前端切到 rust 后端」的验收 —— 页面渲染的数据来自 HTTP 响应
// 与 SSE 帧，而不是测试自己塞进去的对象。
describe('home/latest-activity-card（rust 后端）', () => {
  it('用 HTTP 后端也能渲染推送来的活动', async () => {
    const { createHttpBackend } = await import('@renderer/adapters/http-backend.ts')
    const { installAdapters } = await import('@renderer/adapters/install.ts')

    let dispatchStream: ((event: string, payload: unknown) => void) | undefined
    const requests: Array<{ url: string; method?: string }> = []

    const backend = createHttpBackend({
      runtime: { port: 1, token: 'test-token', pid: 1, version: '0.1.0', started_at: '' },
      fetch: async (url, init) => {
        requests.push({ url, method: init?.method })
        return {
          status: 200,
          ok: true,
          text: async () => JSON.stringify({ code: 0, message: 'ok', data: { success: true } })
        }
      },
      streamFactory: (_dispatch) => {
        dispatchStream = _dispatch
        return { close: () => {} }
      }
    })
    installAdapters({ backend, target: window as unknown as Record<string, unknown> })

    renderCard()
    await waitFor(() => expect(requests.length).toBeGreaterThan(0))

    // daemon 的「打开推送」走的是 POST /api/v1/latest-activity/poll
    expect(requests[0].url).toContain('/api/v1/latest-activity/poll')
    expect(requests[0].method).toBe('POST')

    act(() => dispatchStream?.('push:latest-activity', activity))

    expect(await screen.findByText('季度财报整理')).toBeInTheDocument()
  })
})
