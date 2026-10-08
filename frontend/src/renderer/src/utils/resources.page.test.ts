// F1：`resources` 是后端序列化的 JSON 字符串，坏数据不能让页面崩。
//
// 这类字段在渲染路径上解析：一次 SyntaxError 就是整页白屏（App.tsx 的启动握手
// 出过同类问题）。这里把容错行为钉住，5 个调用点都走它。

import { describe, expect, it } from 'vitest'

import { parseJsonArray, withParsedResources } from './resources'

describe('resources 容错解包', () => {
  it('正常 JSON 数组原样解出', () => {
    expect(parseJsonArray('[{"id":"act-1"}]')).toEqual([{ id: 'act-1' }])
  })

  it('坏 JSON / 缺失 / 空串都返回空数组，不抛异常', () => {
    expect(parseJsonArray('{不是 JSON')).toEqual([])
    expect(parseJsonArray('')).toEqual([])
    expect(parseJsonArray(undefined)).toEqual([])
    expect(parseJsonArray(null)).toEqual([])
  })

  it('是合法 JSON 但不是数组时也返回空数组（形状不对不当成数据）', () => {
    expect(parseJsonArray('{"a":1}')).toEqual([])
    expect(parseJsonArray('"text"')).toEqual([])
    expect(parseJsonArray('123')).toEqual([])
  })

  it('已经是数组时直接用（不重复解析）', () => {
    const rows = [{ id: 'x' }]
    expect(parseJsonArray(rows)).toBe(rows)
  })

  it('整行解包：坏 resources 只丢这一字段，活动本身还在', () => {
    const row = { id: 'act-1', title: '写代码', resources: '坏 JSON' }

    expect(withParsedResources(row)).toEqual({
      id: 'act-1',
      title: '写代码',
      resources: []
    })
  })
})
