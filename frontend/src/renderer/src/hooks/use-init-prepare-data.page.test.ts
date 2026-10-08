import { TaskUrgency } from '@renderer/constant/feed'
import { installFakeBackend } from '@renderer/test/page-setup'
import { renderHook, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import { useInitPrepareData } from './use-init-prepare-data'

it('首次引导任务默认低优先级，不把教学提示标为紧急任务', async () => {
  installFakeBackend({}, { strict: false })
  vi.spyOn(window.screenMonitorAPI, 'getSettings').mockResolvedValue(undefined)
  const save = vi.spyOn(window.screenMonitorAPI, 'setSettings').mockResolvedValue({ success: true })
  const { result } = renderHook(() => useInitPrepareData())
  await waitFor(() => expect(result.current.data).toHaveLength(3))
  expect(result.current.data.every((task) => task.urgency === TaskUrgency.Low)).toBe(true)
  expect(save).toHaveBeenCalledWith(
    'todoList',
    expect.arrayContaining([expect.objectContaining({ urgency: TaskUrgency.Low })])
  )
})
