// 页面级测试：设置页开机自启开关的三条分支。
//
// 重点不是「开关能点」，而是**读不回真值时不许假装知道系统状态**，
// 以及没有外壳时不给出一个点了没反应的控件。最后一条钉住「设置页真的
// 渲染了它」—— 组件写好了却没人用，是这类改动最容易出的问题。

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { LaunchAtLoginSwitch } from './launch-at-login-switch'

function switchEl(): HTMLElement {
  return screen.getByRole('switch')
}

function isOn(): boolean {
  return switchEl().getAttribute('aria-checked') === 'true'
}

function isDisabled(): boolean {
  return switchEl().hasAttribute('disabled') || switchEl().getAttribute('aria-disabled') === 'true'
}

describe('settings/launch-at-login-switch', () => {
  it('Tauri：显示系统实际值，切换后写设置并显示读回结果', async () => {
    const api = {
      shell: 'tauri' as const,
      read: vi.fn().mockResolvedValue(true),
      write: vi.fn().mockResolvedValue(false)
    }
    render(<LaunchAtLoginSwitch api={api} />)

    await waitFor(() => expect(isOn()).toBe(true))
    fireEvent.click(switchEl())

    await waitFor(() => expect(api.write).toHaveBeenCalledWith(false))
    await waitFor(() => expect(isOn()).toBe(false))
  })

  it('外壳写成功但读不回真值 → 明说「未确认」', async () => {
    const api = {
      shell: 'tauri' as const,
      read: vi.fn().mockResolvedValue(undefined),
      write: vi.fn().mockResolvedValue(undefined)
    }
    render(<LaunchAtLoginSwitch api={api} />)

    await waitFor(() => expect(isDisabled()).toBe(false))
    fireEvent.click(switchEl())

    // 文案走词条后按默认语言（中文）断言，避免把英文硬编码进测试
    expect(await screen.findByText(/未确认/)).toBeInTheDocument()
  })

  it('没有外壳实现：开关禁用并说明原因，而不是点了没反应', async () => {
    const api = {
      shell: 'none' as const,
      read: vi.fn().mockResolvedValue(undefined),
      write: vi.fn().mockRejectedValue(new Error('开机自启需要桌面外壳提供（当前外壳未接线）'))
    }
    render(<LaunchAtLoginSwitch api={api} />)

    await waitFor(() => expect(isDisabled()).toBe(true))
    expect(screen.getByText(/不支持开机自启/)).toBeInTheDocument()
    expect(api.write).not.toHaveBeenCalled()
  })

  it('设置页真的渲染了这个开关（防止组件写好了却没人用）', () => {
    // vitest 里 import.meta.url 不是 file: 协议，所以按仓库根相对路径读源码
    const source = readFileSync(resolve(process.cwd(), 'src/renderer/src/pages/settings/settings.tsx'), 'utf8')
    expect(source).toContain('<LaunchAtLoginSwitch')
  })
})
