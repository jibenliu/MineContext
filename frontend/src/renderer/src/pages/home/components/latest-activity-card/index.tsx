// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { useNavigation } from '@renderer/hooks/use-navigation'
import { useI18n } from '@renderer/i18n'
import { ActivityTimelineItem } from '@renderer/pages/screen-monitor/components/activitie-timeline-item'
import { withParsedResources } from '@renderer/utils/resources'
import { useMount, useUnmount } from 'ahooks'
import { isEmpty } from 'lodash'
import { FC, useState } from 'react'

import { CardLayout } from '../layout'

interface LatestActivityCardProps {
  title: string
  hasToDocButton?: boolean
  emptyText?: string
  children?: React.ReactNode
}

const LatestActivityCard: FC<LatestActivityCardProps> = () => {
  const { t } = useI18n()
  const { navigateToMainTab } = useNavigation()

  const handleNavigateToScreenMonitor = () => {
    navigateToMainTab('screen-monitor', '/screen-monitor')
  }

  const [latestActivity, setLatestActivity] = useState<Activity>()
  useMount(() => {
    window.eventLoop.getHomeLatestActivity('running')
    window.serverPushAPI.pushHomeLatestActivity((data) => {
      setLatestActivity(data)
    })
  })
  useUnmount(() => {
    window.eventLoop.getHomeLatestActivity('stopped')
  })

  return (
    <CardLayout
      seeAllClick={handleNavigateToScreenMonitor}
      title={t('home.latestActivity')}
      emptyText={t('home.latestActivity.empty')}
      isEmpty={isEmpty(latestActivity)}>
      {latestActivity ? (
        <ActivityTimelineItem
          activity={{ ...latestActivity, resources: withParsedResources(latestActivity).resources } as any}
        />
      ) : null}
    </CardLayout>
  )
}

export { LatestActivityCard }
