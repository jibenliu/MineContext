import { configureStore } from '@reduxjs/toolkit'
import captureSources from '@renderer/store/capture-sources'
import setting from '@renderer/store/setting'
import { installFakeBackend } from '@renderer/test/page-setup'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { Provider } from 'react-redux'
import { MemoryRouter } from 'react-router-dom'
import { expect, it, vi } from 'vitest'

import ScreenMonitor from './screen-monitor'

const screenHooks = vi.hoisted(() => ({
  hasPermission: true,
  currentSession: null,
  selectedImage: null,
  grantPermission: vi.fn(),
  setSelectedImage: vi.fn(),
  getNewActivities: vi.fn().mockResolvedValue([]),
  getActivitiesByDate: vi.fn().mockResolvedValue([])
}))
vi.mock('@renderer/hooks/use-screen', () => ({ useScreen: () => screenHooks }))

it('挂载后加载采集源，失败可见且重试恢复，不把失败当作空设置写回', async () => {
  const backend = installFakeBackend(
    {
      'screen-monitor:get-capture-all-sources': { success: false, error: 'unavailable' },
      'screen-monitor:get-settings': { screenList: [] },
      'task:check-can-record': { canRecord: true, status: 'stopped' }
    },
    { strict: false }
  )
  const store = configureStore({ reducer: { captureSources, setting } })
  const { unmount } = render(
    <Provider store={store}>
      <MemoryRouter>
        <ScreenMonitor />
      </MemoryRouter>
    </Provider>
  )
  expect(await screen.findByText('采集来源加载失败，请重试')).toBeInTheDocument()
  expect(backend.calls.filter((call) => call.channel === 'task:update-current-record-app')).toHaveLength(0)
  const display = { id: 'screen:1', name: 'Display', type: 'screen', isVisible: true }
  const fetchSources = vi
    .spyOn(window.screenMonitorAPI, 'getCaptureAllSources')
    .mockResolvedValue({ success: true, sources: [display] })
  fireEvent.click(screen.getByRole('button', { name: '重试' }))
  await waitFor(() => expect(store.getState().captureSources.available.state).toBe('hasData'))
  expect(fetchSources).toHaveBeenCalledTimes(1)
  expect(screen.queryByText('采集来源加载失败，请重试')).not.toBeInTheDocument()
  await waitFor(() =>
    expect(backend.calls.some((call) => call.channel === 'task:update-current-record-app')).toBe(true)
  )
  unmount()
})
