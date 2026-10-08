import { installFakeBackend } from '@renderer/test/page-setup'
import { describe, expect, it } from 'vitest'

import { persistEditorImage, resolveEditorImage } from './images'

describe('编辑器本地图片', () => {
  it('保存字节后返回持久文件地址，重开时通过鉴权适配层读取', async () => {
    const backend = installFakeBackend({
      'file:save': { success: true, filePath: '/uploads/note image.png' },
      'file:read': { success: true, data: 'aGk=' }
    })
    const address = await persistEditorImage(new File(['hi'], 'photo.png', { type: 'image/png' }))
    expect(address).toBe('file:///uploads/note%20image.png')
    expect(backend.calls[0].args[0]).toMatch(/^note-image-.+\.png$/)
    expect(backend.calls[0].args[1]).toBe('aGk=')
    expect(await resolveEditorImage(address)).toBe('data:image/png;base64,aGk=')
    expect(backend.calls[1].args).toEqual(['note image.png', 'base64'])
  })

  it('保存失败不返回临时 Blob 地址', async () => {
    installFakeBackend({ 'file:save': { success: false } })
    await expect(persistEditorImage(new File(['hi'], 'photo.png', { type: 'image/png' }))).rejects.toThrow()
  })

  it('拒绝 SVG、过大图片以及已经失效的 Blob 引用', async () => {
    const backend = installFakeBackend({})
    await expect(persistEditorImage(new File(['<svg/>'], 'photo.svg', { type: 'image/svg+xml' }))).rejects.toThrow()
    await expect(
      persistEditorImage(new File([new Uint8Array(1024 * 1024 + 1)], 'photo.png', { type: 'image/png' }))
    ).rejects.toThrow()
    await expect(resolveEditorImage('blob:expired')).rejects.toThrow()
    expect(backend.calls).toHaveLength(0)
  })
})
