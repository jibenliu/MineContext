import { afterEach, expect, it, vi } from 'vitest'

import { writeClipboard } from './write-clipboard'

afterEach(() => {
  vi.unstubAllGlobals()
  document.body.innerHTML = ''
  delete (window as Window & { __TAURI__?: unknown }).__TAURI__
})

it('Tauri 外壳优先走原生 clipboard_write_text', async () => {
  const invoke = vi.fn().mockResolvedValue(undefined)
  ;(window as Window & { __TAURI__?: unknown }).__TAURI__ = { core: { invoke } }
  const writeText = vi.fn().mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })

  await writeClipboard('sk-tauri')

  expect(invoke).toHaveBeenCalledWith('clipboard_write_text', { text: 'sk-tauri' })
  expect(writeText).not.toHaveBeenCalled()
})

it('Tauri 不可用时用 navigator.clipboard.writeText', async () => {
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

it('三种方式都失败时抛错', async () => {
  ;(window as Window & { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: vi.fn().mockRejectedValue(new Error('no acl')) }
  }
  vi.stubGlobal('navigator', {
    clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) }
  })
  Object.defineProperty(document, 'execCommand', {
    configurable: true,
    value: vi.fn().mockReturnValue(false)
  })
  await expect(writeClipboard('x')).rejects.toThrow(/clipboard/)
})
