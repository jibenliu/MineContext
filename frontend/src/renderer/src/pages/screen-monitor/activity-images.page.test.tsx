// 活动时间线里的截图缩略图：必须走受鉴权的读取通道（`readImageAsBase64`）拿 base64。
//
// `file://` 地址在外壳的 webview 里加载不到，界面上就是一片带 alt 的破图 ——
// 而且它把「文件已被保留策略/手工清掉」和「路径本来就拼错了」表现成同一个样子，
// 排查时无从分辨。统计数据卡已经按这条契约走（见 `recording-images.page.test.tsx`），
// 时间线这里也要一致。

import { installFakeBackend } from '@renderer/test/page-setup'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it } from 'vitest'

import { ActivityTimelineItem } from './components/activitie-timeline-item'
import { Activity } from './screen-monitor'

const activity: Activity = {
  id: 'a1',
  start_time: '2026-10-08T15:46:30Z',
  end_time: '2026-10-08T16:00:00Z',
  title: '未知活动',
  content: '',
  resources: [
    { type: 'image', id: 'r1', path: 'screenshots/2026/10/08/one.png' },
    { type: 'image', id: 'r2', path: 'screenshots/2026/10/08/two.png' },
    { type: 'text', id: 'r3', path: 'notes/three.txt' }
  ]
}

it('缩略图经适配层读取，不生成 file:// 地址', async () => {
  const backend = installFakeBackend({
    'screen-monitor:read-image-base64': { data: 'aW1hZ2U=', mime: 'image/png' }
  })
  render(<ActivityTimelineItem activity={activity} />)

  await waitFor(() =>
    expect(screen.getByAltText('screenshot-1')).toHaveAttribute('src', 'data:image/png;base64,aW1hZ2U=')
  )
  // 只读图片类型的两条，路径原样交给适配层（不做 URL 拼接）
  expect(backend.calls.map((call) => call.args[0])).toEqual([
    'screenshots/2026/10/08/one.png',
    'screenshots/2026/10/08/two.png'
  ])
  expect(document.querySelectorAll('img[src^="file:"]')).toHaveLength(0)
})

it('右键菜单能删掉这张截图，只有它消失、别的还在', async () => {
  const backend = installFakeBackend({
    'screen-monitor:read-image-base64': { data: 'aW1hZ2U=', mime: 'image/png' },
    'screen-monitor:delete-screenshot': { success: true }
  })
  render(<ActivityTimelineItem activity={activity} />)

  fireEvent.contextMenu(await screen.findByAltText('screenshot-1'))
  fireEvent.click(await screen.findByText('删除这张截图'))

  await waitFor(() =>
    expect(backend.calls.some((call) => call.channel === 'screen-monitor:delete-screenshot')).toBe(true)
  )
  const deleted = backend.calls.find((call) => call.channel === 'screen-monitor:delete-screenshot')
  expect(deleted?.args[0]).toBe('screenshots/2026/10/08/one.png')
  await waitFor(() => expect(screen.queryByAltText('screenshot-1')).toBeNull())
  expect(screen.getByAltText('screenshot-2')).toBeInTheDocument()
})

it('文件已被清掉时给重试入口，而不是留下一片破图', async () => {
  installFakeBackend({})
  render(<ActivityTimelineItem activity={activity} />)

  // 两条图片资源各给一个重试入口，且不留任何破图占位
  expect(await screen.findAllByRole('button', { name: /重试/ })).toHaveLength(2)
  expect(screen.queryByAltText('screenshot-1')).toBeNull()
  expect(document.querySelectorAll('img[src^="file:"]')).toHaveLength(0)
})
