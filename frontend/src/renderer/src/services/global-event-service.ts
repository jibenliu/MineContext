// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { PushDataTypes } from '@renderer/constant/feed'
import axiosInstance from '@renderer/services/axios-config'
import { addEvent, type EventType } from '@renderer/store/events'
import { Notification } from '@renderer/types/notification'
import { NotificationQueue } from '@renderer/utils/queue/notification-queue'
import { removeMarkdownSymbols } from '@renderer/utils/time'
import { getLogger } from '@shared/logger/renderer'
const logger = getLogger('GlobalEventService')
const NORMAL_POLLING_INTERVAL = 30 * 1000 // Normal: 30 seconds

class GlobalEventService {
  private static instance: GlobalEventService
  private pollingTimer: NodeJS.Timeout | null = null
  private notificationQueue: NotificationQueue
  private fetching = false

  private constructor() {
    this.notificationQueue = NotificationQueue.getInstance()
  }

  public static getInstance(): GlobalEventService {
    if (!GlobalEventService.instance) {
      GlobalEventService.instance = new GlobalEventService()
    }
    return GlobalEventService.instance
  }

  // Start polling
  public startPolling(dispatch): void {
    if (this.pollingTimer) {
      this.stopPolling()
    }
    logger.info('Global event polling started')
    // Execute immediately
    this.fetchEvents(dispatch)

    // Set polling timer
    this.pollingTimer = setInterval(() => {
      this.fetchEvents(dispatch)
    }, NORMAL_POLLING_INTERVAL)
  }

  // Stop polling
  public stopPolling(): void {
    if (this.pollingTimer) {
      logger.info('Global event polling stopped')
      clearInterval(this.pollingTimer)
      this.pollingTimer = null
    }
  }

  // Fetch and process events
  public async fetchEvents(dispatch): Promise<void> {
    if (this.fetching) return
    this.fetching = true
    try {
      const res = await axiosInstance.get('/api/events/fetch')
      if (res.data?.error_code === 'not_implemented') {
        this.stopPolling()
        logger.warn('当前后端不支持旧版全局事件接口，已停止轮询；活动和总结推送使用 SSE。')
        return
      }
      const events = res.data?.data?.events
      if (res.status !== 200 || res.data?.code !== 0 || !Array.isArray(events)) {
        logger.warn('全局事件响应失败或格式无效，本轮未更新事件列表')
        return
      }
      const validEvents = events.filter(
        (event): event is EventType =>
          event !== null &&
          typeof event === 'object' &&
          typeof event.id === 'string' &&
          typeof event.type === 'string' &&
          typeof event.timestamp === 'number' &&
          Number.isFinite(event.timestamp) &&
          event.data !== null &&
          typeof event.data === 'object'
      )
      dispatch(addEvent(validEvents))
      this.processEventsToNotifications(validEvents)
    } catch {
      logger.error('全局事件请求失败，请检查后端连接；下轮将重试')
    } finally {
      this.fetching = false
    }
  }

  // Convert events to notifications
  private processEventsToNotifications(events: any[]): void {
    if (!events || !Array.isArray(events)) {
      return
    }

    events.forEach((event) => {
      if (event.type === PushDataTypes.ACTIVITY_GENERATED) {
        return
      }
      // Create a corresponding notification based on the event type
      const notification: Notification = {
        id: `event-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
        type: this.mapEventTypeToNotificationType(event.type),
        title: this.getEventTitle(event),
        message: removeMarkdownSymbols(typeof event.data.title === 'string' ? event.data.title : '有新的事件通知'),
        timestamp: Date.now(),
        source: 'assistant', // Can be set based on the event source
        channel: 'in-app',
        meta: event // Save the original event data
      }

      // Add the notification to the queue
      this.notificationQueue.add(notification)
    })
  }

  // Map event type to notification type
  private mapEventTypeToNotificationType(eventType: string): Notification['type'] {
    const typeMap: Record<string, Notification['type']> = {
      tip: 'info',
      todo: 'action',
      activity: 'info',
      daily_summary: 'info',
      weekly_summary: 'info',
      system_status: 'warning'
    }
    return typeMap[eventType] || 'info'
  }

  // Get event title
  private getEventTitle(event: any): string {
    const titleMap: Record<string, string> = {
      tip: '提示信息',
      todo: '待办事项',
      activity: '活动通知',
      daily_summary: '每日总结',
      weekly_summary: '每周总结',
      system_status: '系统状态'
    }
    return titleMap[event.type] || '新通知'
  }
}

export default GlobalEventService
