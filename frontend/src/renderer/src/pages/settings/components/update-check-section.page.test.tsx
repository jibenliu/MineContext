// 页面级测试：设置页更新检查。钉住「有更新时能打开链接」与「设置页真的挂了组件」。

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { UpdateCheckSection } from './update-check-section'

describe('settings/update-check-section', () => {
  it('有新版本时展示发布页与 dmg 入口', async () => {
    const api = {
      shell: 'tauri' as const,
      check: vi.fn().mockResolvedValue({
        currentVersion: '1.0.7',
        updateInfo: {
          version: '1.0.8',
          htmlUrl: 'https://github.com/jibenliu/MineContext/releases/tag/v1.0.8',
          dmgUrl: 'https://example.com/MineContext_1.0.8_aarch64.dmg'
        }
      }),
      openUrl: vi.fn().mockResolvedValue(undefined)
    }
    render(<UpdateCheckSection api={api} />)

    fireEvent.click(screen.getByTestId('settings-update-check-button'))
    expect(await screen.findByTestId('settings-update-check-available')).toBeInTheDocument()
    expect(screen.getByText(/1\.0\.8/)).toBeInTheDocument()

    fireEvent.click(screen.getByRole('link', { name: '打开发布页' }))
    await waitFor(() =>
      expect(api.openUrl).toHaveBeenCalledWith(
        'https://github.com/jibenliu/MineContext/releases/tag/v1.0.8'
      )
    )
  })

  it('已是最新时显示 up-to-date 文案', async () => {
    const api = {
      shell: 'tauri' as const,
      check: vi.fn().mockResolvedValue({ currentVersion: '1.0.7', updateInfo: null }),
      openUrl: vi.fn()
    }
    render(<UpdateCheckSection api={api} />)
    fireEvent.click(screen.getByTestId('settings-update-check-button'))
    expect(await screen.findByTestId('settings-update-check-note')).toBeInTheDocument()
    expect(screen.getByText(/最新|up to date/i)).toBeInTheDocument()
  })

  it('没有外壳：按钮禁用并说明原因', async () => {
    const api = {
      shell: 'none' as const,
      check: vi.fn(),
      openUrl: vi.fn()
    }
    render(<UpdateCheckSection api={api} />)
    const button = screen.getByTestId('settings-update-check-button')
    expect(button).toBeDisabled()
    expect(screen.getByText(/不支持检查更新|does not support/i)).toBeInTheDocument()
    expect(api.check).not.toHaveBeenCalled()
  })

  it('设置页真的渲染了这个入口（防止组件写好了却没人用）', () => {
    const source = readFileSync(resolve(process.cwd(), 'src/renderer/src/pages/settings/settings.tsx'), 'utf8')
    expect(source).toContain('<UpdateCheckSection')
  })
})
