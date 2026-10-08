// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import 'allotment/dist/style.css'

import { FC, useEffect, useMemo } from 'react'
import { HashRouter, Route, Routes, useNavigate } from 'react-router-dom'

import { useObservableTask } from './atom/event-loop.atom'
import Sidebar from './components/sidebar'
import { useEvents } from './hooks/use-events'
import AIDemo from './pages/ai-demo/ai-demo'
import { AssistantPage } from './pages/assistant/assistant-page'
import Files from './pages/files/files'
import HomePage from './pages/home/home-page'
import ScreenMonitor from './pages/screen-monitor/screen-monitor'
import { SearchPage } from './pages/search/search-page'
import Settings from './pages/settings/settings'
import { SummariesPage } from './pages/summaries/summaries-page'
import VaultPage from './pages/vault/vault'

const AppContent: FC = () => {
  const navigate = useNavigate()
  const { startPolling, stopPolling } = useEvents()
  useObservableTask({
    active: startPolling,
    inactive: stopPolling
  })

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
        <Route path="/summaries" element={<SummariesPage />} />
        <Route path="/ai-demo" element={<AIDemo />} />
      </Routes>
    )
  }, [])

  return (
    <div className="app-shell-bg flex h-screen" style={{ height: '100vh' }}>
      {/* <div style={{ appRegion: 'drag', width: '12px', height: '100%' } as React.CSSProperties} /> */}
      <Sidebar />
      <div className="flex-1 flex flex-col pr-2">{routes}</div>
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
