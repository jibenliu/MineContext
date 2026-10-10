// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import 'allotment/dist/style.css'

import { FC, useEffect, useMemo } from 'react'
import { HashRouter, Route, Routes, useNavigate } from 'react-router-dom'

import { useObservableTask } from './atom/event-loop.atom'
import FirstRunChecklist from './components/first-run-checklist'
import { useFirstRunChecklist } from './components/first-run-checklist/use-first-run-checklist'
import Sidebar from './components/sidebar'
import { useEvents } from './hooks/use-events'
import { useTrayRecordingSync } from './hooks/use-tray-recording-sync'
import AIDemo from './pages/ai-demo/ai-demo'
import { AssistantPage } from './pages/assistant/assistant-page'
import Files from './pages/files/files'
import HomePage from './pages/home/home-page'
import { InsightsPage } from './pages/insights/insights-page'
import ScreenMonitor from './pages/screen-monitor/screen-monitor'
import { SearchPage } from './pages/search/search-page'
import Settings from './pages/settings/settings'
import { SummariesPage } from './pages/summaries/summaries-page'
import VaultPage from './pages/vault/vault'

const AppContent: FC = () => {
  const navigate = useNavigate()
  const firstRun = useFirstRunChecklist()
  const { startPolling, stopPolling } = useEvents()
  useObservableTask({
    active: startPolling,
    inactive: stopPolling
  })
  // 托盘录制指示：生命周期跟 app shell，不跟屏幕监控页
  useTrayRecordingSync()

  // 托盘菜单「屏幕监控」：点击后导航过去
  useEffect(() => {
    const handleNavigateToScreenMonitor = () => {
      navigate('/screen-monitor')
    }

    return window.api.onTrayNavigateToScreenMonitor(handleNavigateToScreenMonitor)
  }, [navigate])

  // 托盘菜单「开始 / 暂停录制」：导航到屏幕监控页并带上切换信号
  useEffect(() => {
    const handleTrayToggleRecording = () => {
      navigate('/screen-monitor', { state: { toggleRecording: true } })
    }

    return window.api.onTrayToggleRecording(handleTrayToggleRecording)
  }, [navigate])

  useEffect(() => {
    startPolling()

    return () => stopPolling()
  }, [startPolling, stopPolling])

  const routes = useMemo(() => {
    return (
      <Routes>
        <Route path="/" element={<HomePage />} />
        <Route path="/vault" element={<VaultPage />} />
        <Route path="/screen-monitor" element={<ScreenMonitor />} />
        <Route path="/settings" element={<Settings />} />
        <Route path="/files" element={<Files />} />
        <Route path="/search" element={<SearchPage />} />
        <Route path="/assistant" element={<AssistantPage />} />
        <Route path="/insights" element={<InsightsPage />} />
        <Route path="/summaries" element={<SummariesPage />} />
        <Route path="/ai-demo" element={<AIDemo />} />
      </Routes>
    )
  }, [])

  return (
    <div className="app-shell-bg flex h-screen" style={{ height: '100vh' }}>
      {/* <div style={{ appRegion: 'drag', width: '12px', height: '100%' } as React.CSSProperties} /> */}
      <Sidebar />
      {/* min-w-0：主栏 flex 子项默认 min-width:auto 会卡住宽度，助手页铺不满 */}
      <div className="flex min-w-0 flex-1 flex-col pr-2 pt-2">
        {firstRun.visible ? (
          <FirstRunChecklist
            steps={firstRun.steps}
            onRequestPermission={() => void firstRun.requestPermission()}
            onGoApiKey={() => navigate('/settings')}
            onStartRecording={() => navigate('/screen-monitor', { state: { toggleRecording: true } })}
            onClearWaiting={firstRun.clearWaiting}
            onDismiss={firstRun.dismiss}
          />
        ) : null}
        {routes}
      </div>
    </div>
  )
}

const Router: FC = () => {
  return (
    <HashRouter>
      <AppContent />
    </HashRouter>
  )
}

export default Router
