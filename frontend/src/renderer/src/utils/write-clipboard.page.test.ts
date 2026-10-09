import { afterEach, expect, it, vi } from 'vitest'

import { writeClipboard } from './write-clipboard'

afterEach(() => {
  vi.unstubAllGlobals()
  document.body.innerHTML = ''
})

it('优先使用 navigator.clipboard.writeText', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })
  await writeClipboard('sk-secret')
  expect(writeText).toHaveBeenCalledWith('sk-secret')
})

it('clipboard API 失败时回退到 execCommand', async () => {
  vi.stubGlobal('navigator', {
    clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) }
  })
  const exec = vi.fn().mockReturnValue(true)
  Object.defineProperty(document, 'execCommand', { configurable: true, value: exec })
  await writeClipboard('sk-fallback')
  expect(exec).toHaveBeenCalledWith('copy')
})

it('两种方式都失败时抛错', async () => {
  vi.stubGlobal('navigator', {
    clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) }
  })
  Object.defineProperty(document, 'execCommand', {
    configurable: true,
    value: vi.fn().mockReturnValue(false)
  })
  await expect(writeClipboard('x')).rejects.toThrow(/clipboard/)
})
