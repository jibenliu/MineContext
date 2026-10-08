import store from '@renderer/store'
import { setLocale } from '@renderer/store/setting'
import { act, renderHook } from '@testing-library/react'
import { afterEach, expect, it } from 'vitest'

import { useI18n } from './index'

afterEach(() => {
  act(() => store.dispatch(setLocale('zh-CN')))
})

it('同一语言的翻译回调稳定，切换语言更新文案', () => {
  const { result, rerender } = renderHook(() => useI18n())
  const original = result.current.t
  rerender()
  expect(result.current.t).toBe(original)
  act(() => {
    store.dispatch(setLocale('en-US'))
  })
  expect(result.current.t('app.language')).toBe('Language')
  expect(result.current.t).not.toBe(original)
})
