import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { type IndexingApi, IndexingPauseBanner } from './indexing-pause-banner'

afterEach(() => {
  cleanup()
})

function renderBanner(api: IndexingApi) {
  return render(
    <MemoryRouter>
      <IndexingPauseBanner api={api} pollMs={60_000} />
    </MemoryRouter>
  )
}

describe('IndexingPauseBanner', () => {
  it('暂停时展示原因，并可一键恢复', async () => {
    const resume = vi.fn(async () => ({ resumed: true }))
    const status = vi
      .fn()
      .mockResolvedValueOnce({
        paused: true,
        indexing_pause: {
          paused: true,
          code: 'api_key_invalid',
          message: '向量索引已暂停：API Key 无效或已过期。',
          action: { target: 'resume_indexing', label: '恢复索引' }
        }
      })
      .mockResolvedValue({ paused: false, indexing_pause: null })

    renderBanner({ status, resume })

    expect(await screen.findByTestId('indexing-pause-banner')).toHaveTextContent(/暂停/)
    expect(screen.getByTestId('indexing-pause-message')).toHaveTextContent(/API Key/)

    fireEvent.click(screen.getByTestId('indexing-pause-resume'))
    await waitFor(() => expect(resume).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(screen.queryByTestId('indexing-pause-banner')).toBeNull())
  })

  it('未暂停时不渲染横幅', async () => {
    renderBanner({
      status: async () => ({ paused: false, indexing_pause: null }),
      resume: async () => ({ resumed: true })
    })
    await waitFor(() => {
      expect(screen.queryByTestId('indexing-pause-banner')).toBeNull()
    })
  })
})
