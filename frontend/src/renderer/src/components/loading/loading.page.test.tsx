import { fireEvent, render, screen } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import LoadingComponent from './index'

it('后端错误态展示重试按钮，点击会调用 onRetry', () => {
  const onRetry = vi.fn()
  render(<LoadingComponent backendStatus="error" onRetry={onRetry} />)

  expect(screen.getByText('欢迎使用 MineContext')).toBeInTheDocument()
  expect(screen.getByText(/无法连接本地服务/)).toBeInTheDocument()
  fireEvent.click(screen.getByTestId('backend-retry'))
  expect(onRetry).toHaveBeenCalledTimes(1)
})

it('非错误态不渲染重试按钮', () => {
  render(<LoadingComponent backendStatus="starting" />)
  expect(screen.queryByTestId('backend-retry')).toBeNull()
})
