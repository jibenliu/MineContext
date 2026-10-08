// 页面级测试：任意时段总结的前端入口。
//
// 断言四件事：
//   ① 生成前先预览：范围内有多少观测/活动/阶段、预计多少 token —— 用户有权知道
//      这次要花多少钱、有没有东西可总结；
//   ② 范围内没数据时不让生成（否则会得到一份空洞的总结，比不生成更糟）；
//   ③ 长范围走异步作业，界面能显示进度，并且**能在跑完前取消**；
//   ④ 结果带质量标记：模型写的与模板兜底必须能区分。

import { calledChannels, installFakeBackend } from '@renderer/test/page-setup'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import dayjs from 'dayjs'
import { describe, expect, it, vi } from 'vitest'

import { AdhocSummary } from './adhoc-summary'

const preview = (overrides: Record<string, unknown> = {}) => ({
  range: {
    from: '2026-09-30T00:00:00Z',
    to: '2026-09-30T08:00:00Z',
    local_from: '2026-09-30 08:00',
    local_to: '2026-09-30 16:00'
  },
  timezone: 'Asia/Shanghai',
  counts: { observations: 128, blocked_observations: 3, activities: 12, stages: 4 },
  estimated_chunks: 2,
  estimated_tokens: 48_000,
  has_data: true,
  ...overrides
})

const running = { job: { job_id: 'job-1', state: 'running', chunks_done: 1, chunks_total: 2 } }

function renderPanel(handlers: Record<string, unknown>) {
  const backend = installFakeBackend(handlers, { strict: false })
  render(<AdhocSummary />)
  return backend
}

describe('任意时段总结（rust 后端）', () => {
  it('作业深链恢复进度并保留取消操作', async () => {
    window.location.hash = '#/summaries?job_id=job-1&from=2026-10-03&to=2026-10-03'
    try {
      const backend = renderPanel({ 'v1:adhoc-job': running, 'v1:adhoc-job-cancel': { cancel_requested: true } })
      await waitFor(() => expect(screen.getByTestId('adhoc-progress')).toHaveTextContent('1/2'))
      fireEvent.click(screen.getByText('取消'))
      await waitFor(() => expect(calledChannels(backend)).toContain('v1:adhoc-job-cancel'))
      expect(calledChannels(backend)).not.toContain('v1:adhoc-job-submit')
    } finally {
      window.location.hash = ''
    }
  })
  it('默认范围显示在日期时间输入框中', () => {
    renderPanel({})
    expect((screen.getByLabelText('开始时间') as HTMLInputElement).value).toMatch(/^\d{4}-\d{2}-\d{2}T00:00$/)
    expect((screen.getByLabelText('结束时间') as HTMLInputElement).value).toMatch(/^\d{4}-\d{2}-\d{2}T23:59$/)
  })
  it('慢进度请求不重叠，卸载后不再轮询', async () => {
    const backend = installFakeBackend(
      {
        'v1:adhoc-preview': preview(),
        'v1:adhoc-job-submit': { job_id: 'job-1' }
      },
      { strict: false }
    )
    const invoke = backend.invoke.bind(backend)
    let finish!: (value: unknown) => void
    const pending = new Promise((resolve) => {
      finish = resolve
    })
    vi.spyOn(backend, 'invoke').mockImplementation(async (channel, ...args) => {
      if (channel === 'v1:adhoc-job') return pending
      return invoke(channel, ...args)
    })
    const { unmount } = render(<AdhocSummary />)
    fireEvent.click(screen.getByText('预览'))
    await screen.findByTestId('adhoc-preview')
    vi.useFakeTimers()
    try {
      await act(async () => {
        fireEvent.click(screen.getByText('生成总结'))
      })
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2400)
      })
      const polls = () => vi.mocked(backend.invoke).mock.calls.filter(([channel]) => channel === 'v1:adhoc-job')
      expect(polls()).toHaveLength(1)
      unmount()
      await act(async () => {
        finish(running)
        await vi.advanceTimersByTimeAsync(2400)
      })
      expect(polls()).toHaveLength(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('进度连接失败可恢复原作业而不重复提交', async () => {
    const backend = renderPanel({
      'v1:adhoc-preview': preview(),
      'v1:adhoc-job-submit': { job_id: 'job-1' },
      'v1:adhoc-job': { job: { ...running.job, state: 'done' }, summary: { id: 'sum-1', title: '恢复成功' } }
    })
    const invoke = backend.invoke.bind(backend)
    let failed = false
    vi.spyOn(backend, 'invoke').mockImplementation(async (channel, ...args) => {
      if (channel === 'v1:adhoc-job' && !failed) {
        failed = true
        throw new Error('offline')
      }
      return invoke(channel, ...args)
    })
    fireEvent.click(screen.getByText('预览'))
    await screen.findByTestId('adhoc-preview')
    fireEvent.click(screen.getByText('生成总结'))
    expect(await screen.findByRole('alert')).toHaveTextContent('获取进度失败')
    expect(screen.getByText('生成总结').closest('button')).toBeDisabled()
    fireEvent.click(screen.getByText('重试'))
    expect(await screen.findByText('恢复成功')).toBeInTheDocument()
    expect(backend.calls.filter((call) => call.channel === 'v1:adhoc-job-submit')).toHaveLength(1)
  })

  it('修改范围后必须重新预览', async () => {
    renderPanel({ 'v1:adhoc-preview': preview() })
    fireEvent.click(screen.getByText('预览'))
    await screen.findByTestId('adhoc-preview')
    fireEvent.change(screen.getByLabelText('开始时间'), { target: { value: '2026-10-01T12:00' } })
    expect(screen.queryByTestId('adhoc-preview')).toBeNull()
    expect(screen.getByText('生成总结').closest('button')).toBeDisabled()
  })

  it('服务端作业失败显示原因', async () => {
    renderPanel({
      'v1:adhoc-preview': preview(),
      'v1:adhoc-job-submit': { job_id: 'job-1' },
      'v1:adhoc-job': { job: { ...running.job, state: 'failed', error: '模型超时' } }
    })
    fireEvent.click(screen.getByText('预览'))
    await screen.findByTestId('adhoc-preview')
    fireEvent.click(screen.getByText('生成总结'))
    expect(await screen.findByRole('alert')).toHaveTextContent('模型超时')
  })
  it('深链带来的分钟级范围照实呈现，不被折成整天', async () => {
    const from = '2026-10-04T14:20:00+08:00'
    const to = '2026-10-04T15:40:00+08:00'
    window.location.hash = `#/summaries?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`
    try {
      renderPanel({ 'v1:adhoc-preview': preview() })

      const note = await screen.findByTestId('adhoc-precision-range')
      expect(note.textContent).toContain(dayjs(from).format('MM-DD HH:mm'))
      expect(note.textContent).toContain(dayjs(to).format('MM-DD HH:mm'))
    } finally {
      window.location.hash = ''
    }
  })

  it('先预览：看到范围内有什么、要花多少 token', async () => {
    const backend = renderPanel({
      'v1:adhoc-preview': preview(),
      'v1:adhoc-job-submit': { job_id: 'job-1', chunks_total: 2, state: 'running' },
      'v1:adhoc-job': running
    })

    fireEvent.change(screen.getByLabelText('开始时间'), { target: { value: '2026-09-30T00:00' } })
    fireEvent.change(screen.getByLabelText('结束时间'), { target: { value: '2026-09-30T23:59' } })
    fireEvent.click(screen.getByText('预览'))

    expect(await screen.findByTestId('adhoc-preview')).toHaveTextContent('128')
    expect(screen.getByTestId('adhoc-preview')).toHaveTextContent('48000')
    expect(screen.getByTestId('adhoc-preview')).toHaveTextContent('3')
    expect(calledChannels(backend)).toContain('v1:adhoc-preview')
  })

  it('范围内没有数据时不允许生成', async () => {
    renderPanel({
      'v1:adhoc-preview': preview({
        counts: { observations: 0, blocked_observations: 0, activities: 0, stages: 0 },
        has_data: false,
        estimated_tokens: 0
      })
    })

    fireEvent.change(screen.getByLabelText('开始时间'), { target: { value: '2026-09-30T00:00' } })
    fireEvent.change(screen.getByLabelText('结束时间'), { target: { value: '2026-09-30T23:59' } })
    fireEvent.click(screen.getByText('预览'))

    expect(await screen.findByText('这段时间没有可总结的内容')).toBeInTheDocument()
    // Arco 的 Button 把 disabled 放在外层 <button> 上，文案节点是它里面的 span
    expect(screen.getByText('生成总结').closest('button')).toBeDisabled()
  })

  it('生成走异步作业：显示进度并可取消', async () => {
    const backend = renderPanel({
      'v1:adhoc-preview': preview(),
      'v1:adhoc-job-submit': { job_id: 'job-1', chunks_total: 2, state: 'running' },
      'v1:adhoc-job': running,
      'v1:adhoc-job-cancel': { cancelled: true }
    })

    fireEvent.change(screen.getByLabelText('开始时间'), { target: { value: '2026-09-30T00:00' } })
    fireEvent.change(screen.getByLabelText('结束时间'), { target: { value: '2026-09-30T23:59' } })
    fireEvent.click(screen.getByText('预览'))
    await screen.findByTestId('adhoc-preview')

    fireEvent.click(screen.getByText('生成总结'))
    await waitFor(() => expect(calledChannels(backend)).toContain('v1:adhoc-job-submit'))

    // 进度来自服务端，不是本地假进度
    await waitFor(() => expect(screen.getByTestId('adhoc-progress')).toHaveTextContent('1/2'))

    fireEvent.click(screen.getByText('取消'))
    await waitFor(() => expect(calledChannels(backend)).toContain('v1:adhoc-job-cancel'))
  })

  it('完成后显示总结与质量标记（兜底必须看得出来）', async () => {
    renderPanel({
      'v1:adhoc-preview': preview(),
      'v1:adhoc-job-submit': { job_id: 'job-1', chunks_total: 1, state: 'running' },
      'v1:adhoc-job': {
        job: { job_id: 'job-1', state: 'done', chunks_done: 1, chunks_total: 1 },
        summary: {
          id: 'sum-9',
          title: '上午：迁移旧库',
          body_markdown: '把旧库搬到新库，踩了两个坑。',
          quality: 'fallback',
          start: '2026-09-30T00:00:00Z',
          end: '2026-09-30T08:00:00Z'
        }
      }
    })

    fireEvent.change(screen.getByLabelText('开始时间'), { target: { value: '2026-09-30T00:00' } })
    fireEvent.change(screen.getByLabelText('结束时间'), { target: { value: '2026-09-30T23:59' } })
    fireEvent.click(screen.getByText('预览'))
    await screen.findByTestId('adhoc-preview')

    fireEvent.click(screen.getByText('生成总结'))

    expect(await screen.findByText('上午：迁移旧库')).toBeInTheDocument()
    expect(screen.getByTestId('adhoc-quality')).toHaveTextContent('模板兜底')
  })

  it('预设时段：点一下就填好起止日期，再预览走的是同一条路径', async () => {
    const backend = renderPanel({
      'v1:adhoc-preview': preview()
    })

    fireEvent.click(screen.getByTestId('adhoc-preset-最近 7 天'))
    fireEvent.click(screen.getByText('预览'))

    await waitFor(() => expect(calledChannels(backend)).toContain('v1:adhoc-preview'))
    const call = backend.calls.find((entry) => entry.channel === 'v1:adhoc-preview')
    const [from, to] = (call?.args ?? []) as [string, string]
    // 预设填的是「整天」：开始 00:00、结束 23:59（带时刻的输入框只到分钟）
    expect(from).toBe(dayjs().subtract(6, 'day').startOf('day').toISOString())
    expect(dayjs(to).format('YYYY-MM-DD HH:mm')).toBe(dayjs().format('YYYY-MM-DD 23:59'))
    expect(to).toBe(dayjs().endOf('day').toISOString())
  })
})
