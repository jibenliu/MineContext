// 页面级测试：文件页能渲染出后端给的已分析文档，并走导入通道。

import { installFakeBackend } from '@renderer/test/page-setup'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import Files from './files'

// 形状取自 `useFiles`：它读 `result.success` 与 `result.files[].name`
// （不是裸数组，字段也不是 `fileName` —— 形状不对时页面只是「没有文档」，不会报错）。
const DOC = {
  id: 7,
  name: '季度复盘.md',
  size: 2048,
  type: 'text/markdown',
  uploadTime: '2026-10-08 10:00:00'
}

describe('文件页（渲染 + 取数接线）', () => {
  it('已保存文件显示已上传，不宣称分析成功', async () => {
    installFakeBackend(
      { 'file:get-all': { success: true, files: [{ ...DOC, status: 'Uploaded' }] } },
      { strict: false }
    )
    render(<Files />)
    expect(await screen.findByText('已上传')).toBeInTheDocument()
    expect(screen.queryByText('分析成功')).toBeNull()
  })

  it('列表加载失败提供重试', async () => {
    installFakeBackend({}, { strict: false })
    vi.spyOn(window.fileService, 'getFiles')
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({ success: true, files: [DOC] })
    render(<Files />)
    expect(await screen.findByRole('alert')).toHaveTextContent('加载文件失败')
    fireEvent.click(screen.getByText('重试'))
    expect(await screen.findByText(DOC.name)).toBeInTheDocument()
  })

  it('导入失败保留对话框并支持重试，不向页面根路径上传', async () => {
    installFakeBackend({ 'file:get-all': { success: true, files: [] } }, { strict: false })
    const importFile = vi
      .spyOn(window.fileService, 'importFile')
      .mockRejectedValueOnce(new Error('disk full'))
      .mockResolvedValueOnce({
        id: 11,
        title: 'REPORT',
        name: 'REPORT.MD',
        kind: 'unstructured',
        file_path: '/uploads/REPORT.MD'
      })
    const upload = vi.spyOn(XMLHttpRequest.prototype, 'open')
    const { container } = render(<Files />)
    const input = container.querySelector('input[type="file"]')!
    fireEvent.change(input, {
      target: { files: [new File(['hello'], 'REPORT.MD', { type: 'text/markdown' })] }
    })
    fireEvent.click(await screen.findByText('导入并分析'))
    expect(await screen.findByRole('alert')).toHaveTextContent('导入失败')
    fireEvent.click(screen.getByText('导入并分析'))
    expect(await screen.findByText('分析成功')).toBeInTheDocument()
    expect(importFile).toHaveBeenCalledTimes(2)
    expect(upload).not.toHaveBeenCalled()
  })

  it('取一次文件列表并渲染文档名', async () => {
    const backend = installFakeBackend({ 'file:get-all': { success: true, files: [DOC] } }, { strict: false })

    render(<Files />)

    await waitFor(() => expect(backend.calls.filter((call) => call.channel === 'file:get-all')).toHaveLength(1))

    expect(await screen.findByText(DOC.name)).toBeInTheDocument()
  })
})
