import { describe, expect, it } from 'vitest'

import { apiKeyInputShouldBeVisible } from './api-key-visibility'

describe('apiKeyInputShouldBeVisible', () => {
  it('已存脱敏串必须可见（不能盖成 password 圆点）', () => {
    expect(
      apiKeyInputShouldBeVisible({
        hasStoredKey: true,
        maskedValue: 'sk-l••••••••6789',
        fieldValue: 'sk-l••••••••6789',
        userWantsVisible: false
      })
    ).toBe(true)
  })

  it('已存但表单值尚未回填时也保持可见，避免闪成空白', () => {
    expect(
      apiKeyInputShouldBeVisible({
        hasStoredKey: true,
        maskedValue: 'sk-l••••••••6789',
        fieldValue: '',
        userWantsVisible: false
      })
    ).toBe(true)
  })

  it('用户新输入明文时尊重眼睛开关（可隐藏）', () => {
    expect(
      apiKeyInputShouldBeVisible({
        hasStoredKey: true,
        maskedValue: 'sk-l••••••••6789',
        fieldValue: 'sk-brand-new-secret',
        userWantsVisible: false
      })
    ).toBe(false)
  })

  it('从未配置时不强制可见', () => {
    expect(
      apiKeyInputShouldBeVisible({
        hasStoredKey: false,
        maskedValue: '',
        fieldValue: '',
        userWantsVisible: false
      })
    ).toBe(false)
  })
})
