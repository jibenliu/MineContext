// 错误边界：坏数据只让这一块降级，而不是整页白屏。用 vitest + jsdom 跑。

import { render, screen } from '@testing-library/react'
import React from 'react'
import { describe, expect, it, vi } from 'vitest'

import { ErrorBoundary } from './index'

function Boom(): React.ReactElement {
  throw new Error('契约漂移：expected array but got object')
}

describe('ErrorBoundary', () => {
  it('子树抛异常时显示降级提示，而不是把整页留白', () => {
    // React 会往 console.error 打错误栈，这里静音（断言的是用户看到什么）
    const spy = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    render(
      <ErrorBoundary title="总结卡片加载失败">
        <Boom />
      </ErrorBoundary>
    )
    spy.mockRestore()

    expect(screen.getByTestId('error-boundary')).toBeInTheDocument()
    expect(screen.getByText('总结卡片加载失败')).toBeInTheDocument()
    // 原因要显示出来：只写"出错了"等于让人去猜
    expect(screen.getByText(/契约漂移/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument()
  })

  it('没有异常时原样渲染子树', () => {
    render(
      <ErrorBoundary>
        <div>正常内容</div>
      </ErrorBoundary>
    )

    expect(screen.getByText('正常内容')).toBeInTheDocument()
    expect(screen.queryByTestId('error-boundary')).not.toBeInTheDocument()
  })
})
