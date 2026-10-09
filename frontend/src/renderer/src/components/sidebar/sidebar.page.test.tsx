import store from '@renderer/store'
import { render, screen } from '@testing-library/react'
import { Provider } from 'react-redux'
import { MemoryRouter } from 'react-router-dom'
import { expect, it, vi } from 'vitest'

vi.mock('@renderer/components/vault-tree', () => ({
  default: () => <div data-testid="vault-tree-stub" />
}))

import Sidebar from './index'

it('侧栏六个导航图标都走 currentColor（Arco 图标），暗色下不混用黑描边 SVG', () => {
  render(
    <Provider store={store}>
      <MemoryRouter>
        <Sidebar />
      </MemoryRouter>
    </Provider>
  )

  expect(screen.getByText('首页')).toBeInTheDocument()
  expect(screen.getByText('屏幕监控')).toBeInTheDocument()
  expect(screen.getByText('设置')).toBeInTheDocument()

  const sider = document.querySelector('.sidebar-container')
  expect(sider).toBeTruthy()
  const icons = sider!.querySelectorAll('svg.arco-icon')
  // Home / Screen / Search / Assistant / Summaries / Settings = 6
  expect(icons.length).toBeGreaterThanOrEqual(6)
  // 不得再出现硬编码描边的 <img src="*.svg"> 导航图标
  const navImgs = sider!.querySelectorAll(
    '.flex-shrink-0 img[alt="home"], img[alt="screen-monitor"], img[alt="settings"]'
  )
  expect(navImgs.length).toBe(0)
})
