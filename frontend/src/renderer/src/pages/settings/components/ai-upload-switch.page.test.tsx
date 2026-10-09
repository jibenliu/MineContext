import { getPrivacySettings, updatePrivacySettings } from '@renderer/services/privacy'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'

import { AiUploadSwitch } from './ai-upload-switch'

vi.mock('@renderer/services/privacy', () => ({
  getPrivacySettings: vi.fn(),
  updatePrivacySettings: vi.fn()
}))

vi.mock('@arco-design/web-react', async () => {
  const actual = await vi.importActual<typeof import('@arco-design/web-react')>('@arco-design/web-react')
  return {
    ...actual,
    Message: { ...actual.Message, success: vi.fn(), error: vi.fn() }
  }
})

beforeEach(() => {
  vi.mocked(getPrivacySettings).mockResolvedValue({ ai_upload: false, ai_enabled: true })
  vi.mocked(updatePrivacySettings).mockResolvedValue({ ai_upload: true, ai_enabled: true })
})

it('展示允许 AI 出网开关，默认关，打开时写入配置', async () => {
  render(<AiUploadSwitch />)
  const toggle = await screen.findByRole('switch', { name: /允许 AI 出网|AI upload/i })
  expect(toggle).toHaveAttribute('aria-checked', 'false')

  fireEvent.click(toggle)
  await waitFor(() => {
    expect(updatePrivacySettings).toHaveBeenCalledWith({ ai_upload: true })
  })
})
