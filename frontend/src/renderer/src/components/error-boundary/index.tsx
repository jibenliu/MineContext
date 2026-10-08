// 错误边界：一个组件的数据假设被打破时，只让那一块降级 —— 而不是整页白屏。
//
// 为什么必须有：React 在组件抛异常时会卸载整棵树 —— 数据假设被打破时
// （例如后端返回对象、前端当数组，`summaries.map is not a function`）整页会白屏，
// 而真正该发生的是「这张卡片显示失败 + 重试」。崩溃同时写进渲染层日志（落盘可查）。

import { getLocale, translate } from '@renderer/i18n'
import { getLogger } from '@shared/logger/renderer'
import React from 'react'

const logger = getLogger('ErrorBoundary')

interface ErrorBoundaryProps {
  children: React.ReactNode
  /** 降级时显示的标题，说明"哪一块"失败了。 */
  title?: string
}

interface ErrorBoundaryState {
  error: Error | null
  info: React.ErrorInfo | null
}

export class ErrorBoundary extends React.Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null, info: null }

  static getDerivedStateFromError(error: Error): Partial<ErrorBoundaryState> {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    this.setState({ info })
    logger.error('组件渲染失败，已降级为局部提示：', error, info.componentStack)
  }

  render(): React.ReactNode {
    const { error, info } = this.state
    if (!error) return this.props.children

    return (
      <div className="error-boundary" role="alert" data-testid="error-boundary">
        <div className="error-boundary__title">{this.props.title ?? translate(getLocale(), 'common.loadFailed')}</div>
        <div className="error-boundary__detail">{error.message}</div>
        {/* 开发期把组件栈亮出来：定位「哪一块的数据假设被打破」比只看 message 快得多 */}
        {import.meta.env.DEV && info?.componentStack ? (
          <pre className="error-boundary__stack">{info.componentStack}</pre>
        ) : null}
        <button
          type="button"
          className="error-boundary__retry"
          onClick={() => this.setState({ error: null, info: null })}>
          {translate(getLocale(), 'common.retry')}
        </button>
      </div>
    )
  }
}
