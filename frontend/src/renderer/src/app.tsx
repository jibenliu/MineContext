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

import { type BackendBootPhase, loadingStatusForBootPhase, shouldOfferSettingsEscape } from './adapters/backend-boot'
import { fetchInitCheckFromHealth } from './adapters/init-check-health'
import { installHttpBackendFromRuntime } from './adapters/install'
import { shouldShowOnboarding } from './adapters/onboarding'
import { watchSummaryProgress } from './adapters/summary-progress'
import { ServiceProvider } from './atom/event-loop.atom'
import { bootstrapBackend } from './bootstrap-backend'
import { ErrorBoundary } from './components/error-boundary'
import LoadingComponent from './components/loading'
import { NotificationProvider } from './context/notification-provider'
import { useSystemTheme } from './hooks/use-theme'
import Settings from './pages/settings/settings'
import Router from './router'
import { removeStartupSpinner } from './utils/startup-spinner'

const logger = getLogger('App.tsx')

/** 首次 bootstrap 未就绪时，App 内继续等外壳补齐 runtime（勿立刻硬错误）。 */
const RECOVERY_RETRY = { attempts: 40, delayMs: 250 }

// Arco 组件库的界面语言。业务文案目前是中文，因此这里**不能**硬编码英文 ——
// 那会得到「英文按钮/日期控件 + 中文正文」的混杂界面。真正接入 i18n（语言切换 +
// 后端 general.locale）属于产品决定，已登记在 docs/operations.md 的已知缺口一节。
function AppContent({ backendReady: initialReady }: { backendReady: boolean }): React.ReactElement {
  const [showSetting, setShowSetting] = useState<boolean>(true)
  const [bootPhase, setBootPhase] = useState<BackendBootPhase>(() => (initialReady ? 'ready' : 'waiting'))
  const [recoveryNonce, setRecoveryNonce] = useState(0)
  // 本地服务迟迟不起时仍允许进设置：文案已写「从设置继续配置」，不能只剩重试死循环。
  const [enterUiWithoutBackend, setEnterUiWithoutBackend] = useState(false)

  // PersistGate rehydrate 之后本组件才会挂载：这时再拆 index.html 占位，
  // 避免「占位已拆 + React 子树仍空」的 Tauri 白屏窗口。
  useEffect(() => {
    requestAnimationFrame(() => requestAnimationFrame(() => removeStartupSpinner()))
  }, [])

  // 外壳已 wait runtime 再开窗口；此处再等一轮是为了兜住「setup 超时后才写出
  // runtime.json」——那时首次 bootstrap 会失败，但不应立刻显示无法连接。
  // 依赖只有 initialReady / recoveryNonce：把 bootPhase 放进 deps 会在 failed 时
  // 把自己打回 waiting，形成空转。
  useEffect(() => {
    if (initialReady) return
    let cancelled = false
    setBootPhase('waiting')
    void bootstrapBackend({
      loadRuntime: async () => {
        try {
          return (await window.mcRuntime?.get?.()) ?? null
        } catch (error) {
          logger.warn('[mc] 恢复阶段读 runtime 失败，继续重试', error)
          return null
        }
      },
      install: (runtime) => installHttpBackendFromRuntime(runtime),
      onEvent: (event) => logger.info('[mc] recover', event),
      retry: RECOVERY_RETRY
    }).then((result) => {
      if (cancelled) return
      if (result === 'http') {
        logger.info('[mc] 恢复阶段装上 HTTP 后端')
        setBootPhase('ready')
      } else {
        logger.warn('[mc] 恢复阶段仍无 runtime，升为失败态')
        setBootPhase('failed')
      }
    })
    return () => {
      cancelled = true
    }
  }, [initialReady, recoveryNonce])

  useEffect(() => {
    if (bootPhase !== 'ready') return
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
  }, [bootPhase])

  // 后台总结完成时提示一次：作业在服务端跑，用户可能已经切到别的页面。
  // 外壳没有通知能力时不弹窗，只在日志里说明（不影响总结本身）。
  useEffect(() => {
    if (bootPhase !== 'ready') return
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
  }, [bootPhase])

  const closeSetting = useMemoizedFn(() => {
    logger.info('[mc] 关闭引导/设置页，进入主界面')
    setShowSetting(false)
  })

  const retryBackend = useMemoizedFn(() => {
    logger.warn('[mc] 用户重试连接本地服务（再等外壳 runtime，不整页误报）')
    setEnterUiWithoutBackend(false)
    setBootPhase('waiting')
    setRecoveryNonce((n) => n + 1)
  })

  const continueInSettings = useMemoizedFn(() => {
    logger.warn('[mc] 用户跳过等待本地服务，进入设置继续配置')
    setEnterUiWithoutBackend(true)
    setShowSetting(true)
  })

  if (bootPhase !== 'ready' && !enterUiWithoutBackend) {
    const status = loadingStatusForBootPhase(bootPhase)
    const offerEscape = shouldOfferSettingsEscape(bootPhase)
    return (
      <LoadingComponent
        backendStatus={status}
        onRetry={bootPhase === 'failed' ? retryBackend : undefined}
        onContinue={offerEscape ? continueInSettings : undefined}
      />
    )
  }

  return (
    <ErrorBoundary title="界面加载失败">
      {showSetting ? <Settings closeSetting={closeSetting} init /> : <Router />}
    </ErrorBoundary>
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
