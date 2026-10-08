import store from '@renderer/store'
import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, it } from 'vitest'

import { NotificationSwitch } from './notification-switch'

afterEach(() => {
  act(() => store.dispatch({ type: 'settings/setSystemNotificationsEnabled', payload: true }))
})

it('通知开关写入持久化设置并反映当前值', () => {
  render(<NotificationSwitch />)
  const toggle = screen.getByRole('switch')
  expect(toggle).toHaveAttribute('aria-checked', 'true')
  fireEvent.click(toggle)
  expect(toggle).toHaveAttribute('aria-checked', 'false')
  expect(store.getState().setting.systemNotificationsEnabled).toBe(false)
})
