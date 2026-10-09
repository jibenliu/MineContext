import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { BackfillSection, type JobsApi } from './backfill-section'

vi.mock('@arco-design/web-react', async () => {
  const actual = await vi.importActual<typeof import('@arco-design/web-react')>('@arco-design/web-react')
  return {
    ...actual,
    Message: {
      ...actual.Message,
      error: vi.fn(),
      success: vi.fn(),
      warning: vi.fn(),
      info: vi.fn()
    }
  }
})

function renderSection(api: JobsApi) {
  return render(<BackfillSection api={api} />)
}

describe('设置页补推断', () => {
  it('提交后显示已入队并开始查作业状态', async () => {
    const jobStatus = vi.fn().mockResolvedValue({ id: 7, state: 'succeeded' })
    const api: JobsApi = {
      enqueueBackfill: vi.fn().mockResolvedValue({ job_id: 7, deduped: false }),
      jobStatus
    }

    renderSection(api)
    expect(screen.getByTestId('backfill-section')).toBeInTheDocument()

    fireEvent.click(screen.getByText('开始补推断'))
    await waitFor(() => expect(api.enqueueBackfill).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(jobStatus).toHaveBeenCalledWith(7))
    expect(await screen.findByRole('status')).toHaveTextContent('状态：succeeded')
  })

  it('入队失败时提示错误且不开始轮询', async () => {
    const api: JobsApi = {
      enqueueBackfill: vi.fn().mockRejectedValue(new Error('boom')),
      jobStatus: vi.fn()
    }
    renderSection(api)
    fireEvent.click(screen.getByText('开始补推断'))
    await waitFor(() => expect(api.enqueueBackfill).toHaveBeenCalled())
    expect(api.jobStatus).not.toHaveBeenCalled()
  })
})
