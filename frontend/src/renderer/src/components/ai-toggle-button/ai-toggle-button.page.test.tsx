import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import AIToggleButton from './index'

describe('AIToggleButton 对比度', () => {
  it('激活态文字用白字，不用表面色当前景（亮/暗主题都会踩低对比）', () => {
    render(<AIToggleButton isActive onClick={() => undefined} />)
    const button = screen.getByRole('button')
    expect(button.style.color).toMatch(/255,\s*255,\s*255|#fff|white/i)
    expect(button.style.color).not.toContain('color-bg')
  })

  it('未激活态文字用主文字色，不用透明或表面色', () => {
    render(<AIToggleButton isActive={false} onClick={() => undefined} />)
    const button = screen.getByRole('button')
    expect(button.style.color).toContain('color-text-1')
  })
})
