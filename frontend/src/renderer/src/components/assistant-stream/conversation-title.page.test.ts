import { describe, expect, it } from 'vitest'

import { conversationDisplayTitle } from './conversation-title'

describe('conversationDisplayTitle', () => {
  it('有标题时用标题，不回退成 id', () => {
    expect(conversationDisplayTitle({ id: 2, title: '上午的整理' }, '未命名会话')).toBe('上午的整理')
  })

  it('空 / 空白 / null 标题用未命名文案，不用裸数字', () => {
    expect(conversationDisplayTitle({ id: 2, title: null }, '未命名会话')).toBe('未命名会话')
    expect(conversationDisplayTitle({ id: 1, title: '' }, '未命名会话')).toBe('未命名会话')
    expect(conversationDisplayTitle({ id: 1, title: '   ' }, '未命名会话')).toBe('未命名会话')
    expect(conversationDisplayTitle({ id: 1 }, '未命名会话')).toBe('未命名会话')
  })
})
