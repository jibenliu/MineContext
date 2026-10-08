// 页面级测试：系统通知失败必须降级为应用内提示，不能丢成未处理拒绝。
//
// 迁移期两条外壳并存：Tauri 下系统通知桥尚未声明能力，`send` 会显式 reject。
// 这条测试钉住「系统的没了，应用内的还在」，以及「桥整个不存在」这一路。

import { NotificationProvider } from '@renderer/context/notification-provider'
import store from '@renderer/store'
import { setSystemNotificationsEnabled } from '@renderer/store/setting'
import type { Notification as NotificationType } from '@renderer/types/notification'
import { NotificationQueue } from '@renderer/utils/queue/notification-queue'
import { act, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// 应用内提示要「弹出后关闭」才会 resolve，所以假的提示方法必须回调 onClose，
// 否则队列的 Promise.all 永不结束（测试会以超时报错，而不是断言失败）。
const spies = vi.hoisted(() => {
  const make = () => vi.fn((options?: { onClose?: () => void }) => options?.onClose?.())
  return { error: make(), info: make(), success: make(), warning: make(), normal: make() }
})

vi.mock('@arco-design/web-react', () => ({
  Notification: {
    useNotification: () => [spies, null],
    clear: vi.fn()
  }
}))

const notification = {
  id: 'n-1',
  type: 'info',
  title: '标题',
  message: '正文',
  channel: 'system'
} as unknown as NotificationType

type GlobalWithApi = { api?: unknown }

function setApi(value: unknown) {
  ;(window as unknown as GlobalWithApi).api = value
}

let view: ReturnType<typeof render> | undefined

async function mountAndSend() {
  view = render(
    <NotificationProvider>
      <div />
    </NotificationProvider>
  )
  await act(async () => {
    await NotificationQueue.getInstance().add(notification)
  })
}

describe('context/notification-provider', () => {
  beforeEach(() => {
    for (const spy of Object.values(spies)) spy.mockClear()
  })

  afterEach(() => {
    store.dispatch(setSystemNotificationsEnabled(true))
    view?.unmount()
    view = undefined
    delete (window as unknown as GlobalWithApi).api
  })

  it('关闭系统通知后仍保留应用内提示', async () => {
    const send = vi.fn().mockResolvedValue(undefined)
    setApi({ notification: { send } })
    store.dispatch(setSystemNotificationsEnabled(false))
    await mountAndSend()
    expect(send).not.toHaveBeenCalled()
    expect(spies.info).toHaveBeenCalledTimes(1)
  })

  it('系统通知失败时退回应用内提示，且不把失败抛给队列', async () => {
    const send = vi.fn().mockRejectedValue(new Error('notification.send 需要桌面外壳（Tauri）提供，当前未接线'))
    setApi({ notification: { send } })

    await mountAndSend()

    expect(send).toHaveBeenCalledTimes(1)
    expect(spies.info).toHaveBeenCalledTimes(1)
    expect(spies.info.mock.calls[0][0]).toMatchObject({ title: '标题' })
  })

  it('系统通知成功时不再弹应用内提示', async () => {
    const send = vi.fn().mockResolvedValue(undefined)
    setApi({ notification: { send } })

    await mountAndSend()

    expect(send).toHaveBeenCalledTimes(1)
    expect(spies.info).not.toHaveBeenCalled()
  })

  it('外壳桥整个不存在时同样降级，不抛异常也不静默丢弃', async () => {
    setApi(undefined)

    await mountAndSend()

    expect(spies.info).toHaveBeenCalledTimes(1)
  })
})
