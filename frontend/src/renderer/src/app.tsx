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

import { fetchInitCheckFromHealth } from './adapters/init-check-health'
import { shouldShowOnboarding } from './adapters/onboarding'
import { watchSummaryProgress } from './adapters/summary-progress'
import { ServiceProvider } from './atom/event-loop.atom'
import { ErrorBoundary } from './components/error-boundary'
import LoadingComponent from './components/loading'
import { NotificationProvider } from './context/notification-provider'
import { useSystemTheme } from './hooks/use-theme'
import Settings from './pages/settings/settings'
import Router from './router'
import { removeStartupSpinner } from './utils/startup-spinner'

const logger = getLogger('App.tsx')
// Arco 组件库的界面语言。业务文案目前是中文，因此这里**不能**硬编码英文 ——
// 那会得到「英文按钮/日期控件 + 中文正文」的混杂界面。真正接入 i18n（语言切换 +
// 后端 general.locale）属于产品决定，已登记在 docs/operations.md 的已知缺口一节。
function AppContent({ backendReady }: { backendReady: boolean }): React.ReactElement {
  const [showSetting, setShowSetting] = useState<boolean>(true)

  // PersistGate rehydrate 之后本组件才会挂载：这时再拆 index.html 占位，
  // 避免「占位已拆 + React 子树仍空」的 Tauri 白屏窗口。
  useEffect(() => {
    requestAnimationFrame(() => requestAnimationFrame(() => removeStartupSpinner()))
  }, [])

  useEffect(() => {
    // SSE 启动帧 + /api/health 双通道：任一判据到达都更新引导态。
    // 只订 SSE 时，PersistGate 外先开流会丢帧；只拉 health 时，离线桌面更慢。
    const applyInitCheck = (data: unknown, source: string) => {
      const needsOnboarding = shouldShowOnboarding(data)
      logger.info('Init settings data:', { needsOnboarding, source })
      setShowSetting(needsOnboarding)
    }

    const subscribe = window.serverPushAPI?.getInitCheckData
    let unsubscribe: (() => void) | undefined
    if (typeof subscribe === 'function') {
      unsubscribe = subscribe((data) => applyInitCheck(data, 'sse'))
    } else {
      logger.warn('[mc] serverPushAPI.getInitCheckData 未接线，改走 /api/health')
    }

    let cancelled = false
    void fetchInitCheckFromHealth({
      getRuntime: async () => {
        try {
          return (await window.mcRuntime?.get?.()) ?? null
        } catch {
          return null
        }
      },
      fetch: globalThis.fetch.bind(globalThis)
    })
      .then((data) => {
        if (!cancelled) applyInitCheck(data, 'health')
      })
      .catch((error) => {
        if (!cancelled) {
          logger.warn('[mc] /api/health 引导判据拉取失败，保持当前引导态', error)
        }
      })

    return () => {
      cancelled = true
      unsubscribe?.()
    }
  }, [])

  // 后台总结完成时提示一次：作业在服务端跑，用户可能已经切到别的页面。
  // 外壳没有通知能力时不弹窗，只在日志里说明（不影响总结本身）。
  useEffect(() => {
    const summaryProgress = window.serverPushAPI?.summaryProgress
    if (typeof summaryProgress !== 'function') {
      return
    }
    const notify = window.api?.notification?.send
    return watchSummaryProgress(
      (handler) => summaryProgress(handler),
      notify
        ? (notification) =>
            store.getState().setting.systemNotificationsEnabled !== false ? notify(notification) : Promise.resolve()
        : undefined,
      (jobId) => logger.info('后台总结已完成', { jobId }),
      (message, error) => logger.warn(message, error)
    )
  }, [])

  const closeSetting = useMemoizedFn(() => {
    logger.info('[mc] 关闭引导/设置页，进入主界面')
    setShowSetting(false)
  })
  return (
    <>
      {backendReady ? (
        <ErrorBoundary title="界面加载失败">
          {showSetting ? <Settings closeSetting={closeSetting} init /> : <Router />}
        </ErrorBoundary>
      ) : (
        <LoadingComponent
          backendStatus="error"
          onRetry={() => {
            logger.warn('[mc] 后端未就绪，用户点击重试 → 整页重载')
            window.location.reload()
          }}
        />
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
