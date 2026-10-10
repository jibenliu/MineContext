// 引用跳转路径：活动 → 截图时间线，笔记 → vault，总结 → 总结页。

import { describe, expect, it } from 'vitest'

import { citationPath } from './citation-target'

describe('助手引用跳转路径', () => {
  it('笔记 document_id（note-{id}）落到 vault 查询参数', () => {
    expect(citationPath({ document_id: 'note-42', title: '复盘', kind: 'document' })).toBe('/vault?id=42')
    // 历史落库偶发用 kind=note，同样认
    expect(citationPath({ document_id: 'note-7', title: '旧引用', kind: 'note' })).toBe('/vault?id=7')
  })

  it('活动带到 screen-monitor，并带上时间以便切日', () => {
    expect(
      citationPath({ document_id: 'act-7', title: '导入', kind: 'activity', at: 1_725_000_000_000 })
    ).toBe('/screen-monitor?activity=act-7&at=1725000000000')
    expect(citationPath({ document_id: 'act-1', title: '无时间', kind: 'activity' })).toBe(
      '/screen-monitor?activity=act-1'
    )
  })

  it('总结落到总结页', () => {
    expect(citationPath({ document_id: 'sum-9', title: '日总结', kind: 'summary' })).toBe(
      '/summaries?id=sum-9'
    )
  })

  it('未知 kind 或残缺笔记 id 不瞎跳', () => {
    expect(citationPath({ document_id: 'x', title: '?', kind: 'observation' })).toBeNull()
    expect(citationPath({ document_id: 'note-', title: '坏', kind: 'document' })).toBeNull()
    expect(citationPath({ document_id: '42', title: '无前缀', kind: 'document' })).toBeNull()
  })
})
