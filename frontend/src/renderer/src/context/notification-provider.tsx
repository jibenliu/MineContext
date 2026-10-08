// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Notification, NotificationHookReturnType } from '@arco-design/web-react'
import store from '@renderer/store'
import { Notification as NotificationType } from '@renderer/types/notification'
import { NotificationQueue } from '@renderer/utils/queue/notification-queue'
import { isFocused } from '@renderer/utils/window'
import { getLogger } from '@shared/logger/renderer'
import React, { createContext, use, useEffect, useMemo } from 'react'

const logger = getLogger('NotificationProvider')

type NotificationContextType = {
  open: NotificationHookReturnType
  destroy: typeof Notification.clear
}

const typeMap: Record<string, 'info' | 'success' | 'warning' | 'error' | 'normal'> = {
  error: 'error',
  success: 'success',
  warning: 'warning',
  info: 'info',
  normal: 'normal',
  progress: 'info',
  action: 'info'
}

const NotificationContext = createContext<NotificationContextType | undefined>(undefined)

export const NotificationProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [api, contextHolder] = Notification.useNotification({
    maxCount: 3
  })

  useEffect(() => {
    const queue = NotificationQueue.getInstance()

    const showInApp = (notification: NotificationType) =>
      new Promise<void>((resolve) => {
        const notificationMethod = api[typeMap[notification.type] || 'info'] as any
        notificationMethod({
          title: notification.title,
          content: notification.message.length > 50 ? notification.message.slice(0, 47) + '...' : notification.message,
          duration: 30000,
          requiredConfirm: true,
          requireInteraction: true,
          position: 'topRight',
          id: notification.id,
          onClose: resolve
        })
      })

    const listener = async (notification: NotificationType) => {
      // 需要系统通知时先交给外壳；外壳没接线（例如 Tauri 迁移期）就退回应用内提示。
      // 通知是「错过就没了」的信息：桥不存在、调用失败都必须降级，不能静默丢弃。
      if (
        store.getState().setting.systemNotificationsEnabled !== false &&
        (notification.channel === 'system' || !isFocused())
      ) {
        const bridge = window.api?.notification
        if (typeof bridge?.send === 'function') {
          try {
            await bridge.send(notification)
            return
          } catch (error) {
            logger.error('[mc] 系统通知失败，改为应用内提示：', error)
          }
        } else {
          logger.error('[mc] 系统通知桥不存在，改为应用内提示')
        }
      }
      return showInApp(notification)
    }
    queue.subscribe(listener)
    return () => queue.unsubscribe(listener)
  }, [api])

  const value = useMemo(
    () => ({
      open: api,
      destroy: Notification.clear
    }),
    [api]
  )

  return (
    <NotificationContext value={value}>
      {contextHolder}
      {children}
    </NotificationContext>
  )
}

export const useNotification = () => {
  const ctx = use(NotificationContext)
  if (!ctx) throw new Error('useNotification must be used within a NotificationProvider')
  return ctx
}
