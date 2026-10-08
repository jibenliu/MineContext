// 页面级测试：搜索页。
//
// 断言三件事：① 关键词发到 `v1:search`；② 命中渲染出标题与类型；
// ③ 「还没搜」与「没有匹配」是两种不同文案 —— 混在一起用户会以为搜索坏了。

import { calledChannels, installFakeBackend } from '@renderer/test/page-setup'
import { act, render, screen, waitFor } from '@testing-library/react'
import { fireEvent } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { rangeBound, SearchResults } from './search-results'

const hits = [{ id: 'act-1', kind: 'activity', title: '写导入脚本', snippet: '旧库 → 新版', score: 3.2 }]

describe('搜索页（rust 后端）', () => {
  it('结果分页且新搜索回到第一页', async () => {
    const rows = Array.from({ length: 21 }, (_, index) => ({
      id: `hit-${index}`,
      kind: 'activity',
      title: `结果 ${index}`
    }))
    installFakeBackend({ 'v1:search': { results: rows } }, { strict: false })
    render(<SearchResults />)
    fireEvent.change(screen.getByLabelText('搜索关键词'), { target: { value: '结果' } })
    fireEvent.click(screen.getByText('搜索'))
    await waitFor(() => expect(screen.getAllByTestId('search-hit')).toHaveLength(20))
    fireEvent.click(screen.getByRole('button', { name: '下一页' }))
    expect(screen.getAllByTestId('search-hit')).toHaveLength(1)
    expect(screen.getByTestId('search-hit')).toHaveTextContent('结果 20')
    fireEvent.click(screen.getByText('搜索'))
    await waitFor(() => expect(screen.getAllByTestId('search-hit')).toHaveLength(20))
  })
  it('慢请求期间显示加载状态且回车不重复请求', async () => {
    const backend = installFakeBackend({}, { strict: false })
    let finish!: (value: unknown) => void
    const query = vi.spyOn(backend, 'invoke').mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        })
    )
    render(<SearchResults />)
    fireEvent.change(screen.getByLabelText('搜索关键词'), { target: { value: '导入' } })
    fireEvent.keyDown(screen.getByLabelText('搜索关键词'), { key: 'Enter' })
    fireEvent.keyDown(screen.getByLabelText('搜索关键词'), { key: 'Enter' })
    expect(screen.getByRole('status')).toHaveTextContent('正在搜索')
    expect(query).toHaveBeenCalledTimes(1)
    await act(async () => {
      finish({ results: hits })
    })
    expect(await screen.findByTestId('search-hit')).toBeInTheDocument()
  })
  it('搜索失败可见并且可以重试', async () => {
    const backend = installFakeBackend({}, { strict: false })
    const query = vi
      .spyOn(backend, 'invoke')
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({ results: hits })
    render(<SearchResults />)
    fireEvent.change(screen.getByLabelText('搜索关键词'), { target: { value: '导入' } })
    fireEvent.click(screen.getByText('搜索'))
    expect(await screen.findByRole('alert')).toHaveTextContent('搜索失败')
    fireEvent.click(screen.getByText('搜索'))
    expect(await screen.findByTestId('search-hit')).toHaveTextContent('写导入脚本')
    expect(query).toHaveBeenCalledTimes(2)
  })

  it('倒置时间范围不发请求', async () => {
    const backend = installFakeBackend({}, { strict: false })
    render(<SearchResults />)
    fireEvent.change(screen.getByLabelText('搜索关键词'), { target: { value: '导入' } })
    fireEvent.change(screen.getByLabelText('开始时间'), { target: { value: '2026-10-08T12:00' } })
    fireEvent.change(screen.getByLabelText('结束时间'), { target: { value: '2026-10-07T12:00' } })
    fireEvent.click(screen.getByText('搜索'))
    expect(await screen.findByRole('alert')).toHaveTextContent('开始时间')
    expect(calledChannels(backend)).not.toContain('v1:search')
  })
  it('输入关键词后展示命中（5.40）', async () => {
    const backend = installFakeBackend({ 'v1:search': { query: '导入', results: hits } }, { strict: false })

    render(<SearchResults />)
    fireEvent.change(screen.getByLabelText('搜索关键词'), { target: { value: '导入' } })
    fireEvent.keyDown(screen.getByLabelText('搜索关键词'), { key: 'Enter' })

    expect(await screen.findByTestId('search-hit')).toHaveTextContent('写导入脚本')
    expect(screen.getByTestId('search-hit')).toHaveTextContent('activity')

    await waitFor(() => expect(calledChannels(backend)).toContain('v1:search'))
  })

  it('没有命中时给可读空态，而不是空白（5.40）', async () => {
    installFakeBackend({ 'v1:search': { query: '不存在', results: [] } }, { strict: false })

    render(<SearchResults />)
    fireEvent.change(screen.getByLabelText('搜索关键词'), { target: { value: '不存在' } })
    fireEvent.keyDown(screen.getByLabelText('搜索关键词'), { key: 'Enter' })

    expect(await screen.findByText('没有匹配的内容')).toBeInTheDocument()
  })

  it('时间窗发给服务端（5.40：服务端支持，前端也要能设）', async () => {
    const backend = installFakeBackend({ 'v1:search': { query: '导入', results: hits } }, { strict: false })

    render(<SearchResults />)
    fireEvent.change(screen.getByLabelText('搜索关键词'), { target: { value: '导入' } })
    // 时间框现在是 `datetime-local`：用户能选到分钟，选了什么就按什么筛
    fireEvent.change(screen.getByLabelText('开始时间'), { target: { value: '2026-09-01T09:30' } })
    fireEvent.change(screen.getByLabelText('结束时间'), { target: { value: '2026-09-30T18:00' } })
    fireEvent.click(screen.getByText('搜索'))

    await waitFor(() => {
      const call = backend.calls.find((item) => item.channel === 'v1:search')
      expect(call, `没有调用 v1:search：${calledChannels(backend).join(', ')}`).toBeTruthy()
      const [query, start, end] = call!.args as [string, number, number]
      expect(query).toBe('导入')
      const from = new Date(start)
      expect([from.getFullYear(), from.getMonth() + 1, from.getDate(), from.getHours(), from.getMinutes()]).toEqual([
        2026, 9, 1, 9, 30
      ])
      const to = new Date(end)
      expect([to.getDate(), to.getHours(), to.getMinutes()]).toEqual([30, 18, 0])
    })
  })

  it('只给日期时按整天算（起点 0 点、终点当天最后一毫秒）', () => {
    expect(new Date(rangeBound('2026-09-01', 'start')!).getHours()).toBe(0)
    expect(new Date(rangeBound('2026-09-30', 'end')!).getHours()).toBe(23)
    expect(rangeBound('', 'start')).toBeUndefined()
  })

  it('还没搜索时不显示「没有匹配」（两者必须能区分）', async () => {
    installFakeBackend({}, { strict: false })

    render(<SearchResults />)

    expect(screen.queryByText('没有匹配的内容')).toBeNull()
    expect(screen.getByText(/输入关键词/)).toBeInTheDocument()
  })
})
