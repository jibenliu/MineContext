import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

vi.mock('@renderer/components/assistant-stream/assistant-stream', () => ({
  AssistantStream: () => <div data-testid="assistant-stream-stub" />
}))

import { AssistantPage } from './assistant-page'

describe('AssistantPage 布局', () => {
  it('主卡片与流式区带全宽 / min-w-0，侧栏右侧能铺满', () => {
    const { container } = render(<AssistantPage />)
    const page = container.querySelector('.assistant-page')
    const surface = container.querySelector('.assistant-page-surface')
    expect(page?.className).toMatch(/\bw-full\b/)
    expect(page?.className).toMatch(/\bmin-w-0\b/)
    expect(page?.className).toMatch(/\bflex-1\b/)
    expect(surface?.className).toMatch(/\bw-full\b/)
    expect(surface?.className).toMatch(/\bmin-w-0\b/)
    expect(surface?.className).toMatch(/\bflex-1\b/)
    expect(screen.getByTestId('assistant-stream-stub')).toBeInTheDocument()
  })
})
