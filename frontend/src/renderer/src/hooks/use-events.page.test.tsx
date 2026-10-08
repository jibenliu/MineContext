import store from '@renderer/store'
import { renderHook } from '@testing-library/react'
import { PropsWithChildren } from 'react'
import { Provider } from 'react-redux'
import { expect, it } from 'vitest'

import { useEvents } from './use-events'

it('普通重渲染不改变轮询回调或派生事件列表', () => {
  const wrapper = ({ children }: PropsWithChildren) => <Provider store={store}>{children}</Provider>
  const { result, rerender } = renderHook(() => useEvents(), { wrapper })
  const original = result.current
  rerender()
  expect(result.current.startPolling).toBe(original.startPolling)
  expect(result.current.stopPolling).toBe(original.stopPolling)
  expect(result.current.feedEvents).toBe(original.feedEvents)
})
