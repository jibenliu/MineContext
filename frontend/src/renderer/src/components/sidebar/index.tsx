// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import './index.css'

import { Layout } from '@arco-design/web-react'
// 六个导航入口统一用 Arco 图标：继承侧栏文字色（currentColor），暗色主题下不会
// 再出现「半黑 SVG + 亮白图标」混排。
import { IconCalendar, IconDesktop, IconHome, IconRobot, IconSearch, IconSettings } from '@arco-design/web-react/icon'
import { ErrorBoundary } from '@renderer/components/error-boundary'
import VaultTree from '@renderer/components/vault-tree'
import { useNavigation } from '@renderer/hooks/use-navigation'
import { useI18n } from '@renderer/i18n'
import { CSSProperties } from 'react'

import logo from '/src/assets/icons/logo.svg'
const { Sider } = Layout

const iconStyle = { width: 16, height: 16 }

const tabItems = [
  {
    key: 'home',
    icon: <IconHome style={iconStyle} />,
    labelKey: 'sidebar.home',
    path: '/'
  },
  {
    key: 'screen-monitor',
    icon: <IconDesktop style={iconStyle} />,
    labelKey: 'sidebar.screenMonitor',
    path: '/screen-monitor'
  },
  {
    key: 'search',
    icon: <IconSearch style={iconStyle} />,
    labelKey: 'sidebar.search',
    path: '/search'
  },
  {
    key: 'assistant',
    icon: <IconRobot style={iconStyle} />,
    labelKey: 'sidebar.assistant',
    path: '/assistant'
  },
  {
    key: 'summaries',
    icon: <IconCalendar style={iconStyle} />,
    labelKey: 'sidebar.summaries',
    path: '/summaries'
  },
  {
    key: 'settings',
    icon: <IconSettings style={iconStyle} />,
    labelKey: 'sidebar.settings',
    path: '/settings'
  }
]

const Sidebar = () => {
  const { t } = useI18n()
  const { navigateToMainTab, isMainTabActive } = useNavigation()

  const handleTabChange = (key: string) => {
    const item = tabItems.find((item) => item.key === key)
    if (item) {
      navigateToMainTab(key, item.path)
    }
  }

  return (
    <Sider
      width={176}
      className="sidebar-container [&_.arco-layout-sider]: !flex !flex-col !bg-transparent !height-[100vh] !px-[12px]"
      style={{ appRegion: 'drag' } as CSSProperties}>
      {/* Top logo and title */}
      <div style={{ height: '16px', appRegion: 'drag' } as React.CSSProperties} />
      <div className="flex items-center px-4 py-2 h-[80px] flex-shrink-0">
        <div className="flex items-center gap-2 flex-1">
          <img
            src={logo}
            alt="Logo"
            style={{
              width: 24,
              height: 24
            }}
          />
          <div
            onClick={() => navigateToMainTab('home', '/')}
            className="text-base font-bold text-[var(--color-text-1)] m-0 whitespace-nowrap cursor-pointer">
            MineContext
          </div>
        </div>
      </div>

      {/* Tab navigation */}
      <div className="flex-shrink-0" style={{ appRegion: 'no-drag' } as CSSProperties}>
        {tabItems.map((item) => (
          <div
            key={item.key}
            onClick={() => handleTabChange(item.key)}
            className={`h-[28px] p-2 px-3 text-left cursor-pointer text-sm leading-[22px]
              ${
                isMainTabActive(item.key)
                  ? 'bg-[var(--color-primary-light-1)] font-medium text-[rgb(var(--primary-6))] hover:bg-[var(--color-primary-light-2)]'
                  : 'bg-transparent font-normal text-[var(--color-text-2)] hover:bg-[var(--color-fill-2)]'
              }
              transition-all duration-200 ease-in-out rounded-lg flex items-center gap-2 mt-[5px]`}>
            <span className="sidebar-nav-icon flex">{item.icon}</span>
            {t(item.labelKey)}
          </div>
        ))}
      </div>

      {/* 树库（react-arborist）在边界数据下会抛：只让笔记树降级，不拖垮整个侧边栏 */}
      <ErrorBoundary title={t('sidebar.vaultLoadFailed')}>
        <VaultTree className="flex-1" />
      </ErrorBoundary>
    </Sider>
  )
}

export default Sidebar
