// 页面级测试：设置页诊断导出三条分支。
//
// 钉住「能导出并提示路径」「失败有反馈」「没有外壳时禁用且设置页真的挂了组件」。

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { Message } from '@arco-design/web-react'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { DiagnosticExportSection } from './diagnostic-export-section'

vi.mock('@arco-design/web-react', async () => {
  const actual = await vi.importActual<typeof import('@arco-design/web-react')>('@arco-design/web-react')
  return {
    ...actual,
    Message: {
      ...actual.Message,
      success: vi.fn(),
      error: vi.fn()
    }
  }
})

describe('settings/diagnostic-export-section', () => {
  beforeEach(() => {
    vi.mocked(Message.success).mockReset()
    vi.mocked(Message.error).mockReset()
  })

  it('Tauri：点击后导出并提示 zip 路径', async () => {
    const api = {
      shell: 'tauri' as const,
      export: vi.fn().mockResolvedValue({
        path: '/tmp/MineContext-diagnostics.zip',
        folder: '/tmp/MineContext-diagnostics'
      })
    }
    render(<DiagnosticExportSection api={api} />)

    fireEvent.click(screen.getByTestId('diagnostic-export-button'))
    await waitFor(() => expect(api.export).toHaveBeenCalled())
    await waitFor(() => expect(Message.success).toHaveBeenCalled())
    const args = vi.mocked(Message.success).mock.calls[0]?.[0]
    expect(String(args)).toContain('MineContext-diagnostics.zip')
  })

  it('导出失败 → 明确错误提示', async () => {
    const api = {
      shell: 'tauri' as const,
      export: vi.fn().mockRejectedValue(new Error('disk full'))
    }
    render(<DiagnosticExportSection api={api} />)

    fireEvent.click(screen.getByTestId('diagnostic-export-button'))
    await waitFor(() => expect(Message.error).toHaveBeenCalled())
  })

  it('没有外壳实现：按钮禁用并说明原因', async () => {
    const api = {
      shell: 'none' as const,
      export: vi.fn().mockRejectedValue(new Error('no shell'))
    }
    render(<DiagnosticExportSection api={api} />)

    const button = screen.getByTestId('diagnostic-export-button')
    expect(button).toBeDisabled()
    expect(screen.getByText(/需要桌面外壳|desktop shell/i)).toBeInTheDocument()
    expect(api.export).not.toHaveBeenCalled()
  })

  it('设置页真的渲染了这个入口（防止组件写好了却没人用）', () => {
    const source = readFileSync(resolve(process.cwd(), 'src/renderer/src/pages/settings/settings.tsx'), 'utf8')
    expect(source).toContain('<DiagnosticExportSection')
  })
})
