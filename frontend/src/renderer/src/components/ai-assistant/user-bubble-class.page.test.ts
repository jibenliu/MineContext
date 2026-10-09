import { describe, expect, it } from 'vitest'

import { USER_BUBBLE_CLASS } from './user-bubble-class'

describe('USER_BUBBLE_CLASS', () => {
  it('浅品牌底配主文字色，禁止 text-white / 破损 class', () => {
    expect(USER_BUBBLE_CLASS).toContain('bg-[var(--color-primary-light-1)]')
    expect(USER_BUBBLE_CLASS).toContain('text-[var(--color-text-1)]')
    expect(USER_BUBBLE_CLASS).not.toContain('text-white')
    expect(USER_BUBBLE_CLASS).not.toMatch(/primary-light-1\]0/)
  })
})
