import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { VaultBackupSection } from './vault-backup-section'

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

vi.mock('@renderer/services/vault-backup', async () => {
  const actual = await vi.importActual<typeof import('@renderer/services/vault-backup')>(
    '@renderer/services/vault-backup'
  )
  return {
    ...actual,
    downloadBlob: vi.fn()
  }
})

describe('设置页数据备份', () => {
  it('导出成功后提示完成', async () => {
    const api = {
      exportZip: vi.fn().mockResolvedValue(new Blob([new Uint8Array([1, 2, 3])])),
      importZipBase64: vi.fn()
    }
    render(<VaultBackupSection api={api} />)
    expect(screen.getByTestId('vault-backup-section')).toBeInTheDocument()
    fireEvent.click(screen.getByTestId('vault-backup-export'))
    await waitFor(() => expect(api.exportZip).toHaveBeenCalledTimes(1))
    expect(await screen.findByRole('status')).toHaveTextContent('已开始下载备份')
  })

  it('导出失败时提示错误', async () => {
    const { Message } = await import('@arco-design/web-react')
    const api = {
      exportZip: vi.fn().mockRejectedValue(new Error('boom')),
      importZipBase64: vi.fn()
    }
    render(<VaultBackupSection api={api} />)
    fireEvent.click(screen.getByTestId('vault-backup-export'))
    await waitFor(() => expect(api.exportZip).toHaveBeenCalled())
    expect(Message.error).toHaveBeenCalled()
  })
})
