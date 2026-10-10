import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { formatApproxBytes, RetentionSection, type RetentionApi } from './retention-section'

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

function renderSection(api: RetentionApi) {
  return render(<RetentionSection api={api} />)
}

describe('设置页截图占用', () => {
  it('加载后显示近似磁盘占用与策略值', async () => {
    const api: RetentionApi = {
      getConfig: vi.fn().mockResolvedValue({
        retentionDays: 7,
        maxTotalGb: 10,
        diskUsage: { totalBytes: 1536 * 1024 * 1024, blobCount: 42 }
      }),
      saveConfig: vi.fn()
    }

    renderSection(api)
    expect(screen.getByTestId('retention-section')).toBeInTheDocument()
    await waitFor(() => expect(api.getConfig).toHaveBeenCalled())
    expect(await screen.findByTestId('retention-disk-usage')).toHaveTextContent(/1\.5\s*GB/)
    expect(screen.getByTestId('retention-disk-usage')).toHaveTextContent('42')
  })

  it('保存时把当前天数与容量写回配置', async () => {
    const saveConfig = vi.fn().mockResolvedValue({ success: true })
    const api: RetentionApi = {
      getConfig: vi.fn().mockResolvedValue({
        retentionDays: 14,
        maxTotalGb: 5,
        diskUsage: { totalBytes: 100, blobCount: 1 }
      }),
      saveConfig
    }

    renderSection(api)
    await screen.findByTestId('retention-disk-usage')
    fireEvent.click(screen.getByText('保存占用策略'))

    await waitFor(() =>
      expect(saveConfig).toHaveBeenCalledWith({
        retentionDays: 14,
        maxTotalGb: 5
      })
    )
  })

  it('formatApproxBytes 给出可读近似值', () => {
    expect(formatApproxBytes(0, 'zh')).toBe('0 B')
    expect(formatApproxBytes(1536 * 1024 * 1024, 'en')).toMatch(/1\.5\s*GB/)
  })
})
