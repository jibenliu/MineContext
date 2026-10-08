// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import '@renderer/assets/main.css'
import '@renderer/assets/theme/index.less'
import 'allotment/dist/style.css'

import { ConfigProvider } from '@arco-design/web-react'
import enUS from '@arco-design/web-react/es/locale/en-US'
import zhCN from '@arco-design/web-react/es/locale/zh-CN'
import store, { persistor } from '@renderer/store'
import { getLogger } from '@shared/logger/renderer'
import { useMemoizedFn } from 'ahooks'
import React, { useEffect, useState } from 'react'
import { Provider } from 'react-redux'
import { PersistGate } from 'redux-persist/integration/react'

import { shouldShowOnboarding } from './adapters/onboarding'
import { watchSummaryProgress } from './adapters/summary-progress'
import { ServiceProvider } from './atom/event-loop.atom'
import { ErrorBoundary } from './components/error-boundary'
import LoadingComponent from './components/loading'
import { NotificationProvider } from './context/notification-provider'
import { useSystemTheme } from './hooks/use-theme'
import Settings from './pages/settings/settings'
import Router from './router'

const logger = getLogger('App.tsx')
// Arco 组件库的界面语言。业务文案目前是中文，因此这里**不能**硬编码英文 ——
// 那会得到「英文按钮/日期控件 + 中文正文」的混杂界面。真正接入 i18n（语言切换 +
// 后端 general.locale）属于产品决定，已登记在 docs/operations.md 的已知缺口一节。
function AppContent({ backendReady }: { backendReady: boolean }): React.ReactElement {
  const [showSetting, setShowSetting] = useState<boolean>(true)

  useEffect(() => {
    window.serverPushAPI.getInitCheckData((data) => {
      // 判据在 `adapters/onboarding.ts`（纯函数、有测试）：`llm` 在现行契约里是
      // 对象而不是布尔值，直接 `!llm` 会把未配置模型的用户放进主界面。
      // 坏数据也不会抛：解析失败按「需要引导」处理，界面不会白屏。
      const needsOnboarding = shouldShowOnboarding(data)
      logger.info('Init settings data:', { needsOnboarding })
      setShowSetting(needsOnboarding)
    })
  }, [])

  // 后台总结完成时提示一次：作业在服务端跑，用户可能已经切到别的页面。
  // 外壳没有通知能力时不弹窗，只在日志里说明（不影响总结本身）。
  useEffect(() => {
    const notify = window.api?.notification?.send
    return watchSummaryProgress(
      (handler) => window.serverPushAPI.summaryProgress(handler),
      notify
        ? (notification) =>
            store.getState().setting.systemNotificationsEnabled !== false ? notify(notification) : Promise.resolve()
        : undefined,
      (jobId) => logger.info('后台总结已完成', { jobId }),
      (message, error) => logger.warn(message, error)
    )
  }, [])

  const closeSetting = useMemoizedFn(() => {
    setShowSetting(false)
  })
  return (
    <>
      {backendReady ? (
        <ErrorBoundary title="界面加载失败">
          {showSetting ? <Settings closeSetting={closeSetting} init /> : <Router />}
        </ErrorBoundary>
      ) : (
        <LoadingComponent backendStatus="error" />
      )}
    </>
  )
}

function App({ backendReady }: { backendReady: boolean }): React.ReactElement {
  // 跟随系统亮/暗（与语言相互独立）
  useSystemTheme()
  // 界面语言：`ConfigProvider` 在 redux Provider 之外，这里直接订阅 store ——
  // 切换语言时整棵树重渲染，Arco 组件语言与业务文案一起变。
  const [locale, setLocale] = useState(
    () => (store.getState().setting as { locale?: 'zh-CN' | 'en-US' }).locale ?? 'zh-CN'
  )
  useEffect(
    () =>
      store.subscribe(() => {
        const next = (store.getState().setting as { locale?: 'zh-CN' | 'en-US' }).locale ?? 'zh-CN'
        setLocale((previous) => (previous === next ? previous : next))
      }),
    []
  )
  return (
    <Provider store={store}>
      <ConfigProvider locale={locale === 'en-US' ? enUS : zhCN}>
        <ServiceProvider>
          <NotificationProvider>
            <PersistGate loading={null} persistor={persistor}>
              <AppContent backendReady={backendReady} />
            </PersistGate>
          </NotificationProvider>
        </ServiceProvider>
      </ConfigProvider>
    </Provider>
  )
}

export default App
