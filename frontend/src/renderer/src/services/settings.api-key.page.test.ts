import { expect, it } from 'vitest'

import { isPlainApiKeyCandidate } from './settings'

it('空串与脱敏回显不算可复制明文', () => {
  expect(isPlainApiKeyCandidate('', 'sk-a••••••••wxyz')).toBe(false)
  expect(isPlainApiKeyCandidate('sk-a••••••••wxyz', 'sk-a••••••••wxyz')).toBe(false)
  expect(isPlainApiKeyCandidate('sk-ab••••••••yz', 'other')).toBe(false)
})

it('刚输入的真实密钥可直接复制', () => {
  expect(isPlainApiKeyCandidate('sk-live-secret-key', 'sk-l••••••••cret')).toBe(true)
  expect(isPlainApiKeyCandidate('sk-live-secret-key', '')).toBe(true)
})
