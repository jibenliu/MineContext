// 页面级测试：时间线上的「这条结论是怎么来的」与用户改名。
//
// 交付上要紧的是两件事：用户能分清「观察到的事实」与「模型的推测」，并且改错了能自己改回来。
//
// 断言四件事：
//   ① 模型推测的活动带「推测」标记，规则命中的带「规则」标记，纯观察的不标；
//   ② 标记里能看到是哪个模型猜的（换模型后要能对比可信度）；
//   ③ 改名走 `v1:activity-override`，参数是 **v1 的字符串 id** 与新标题 ——
//      拿兼容层的数字 id 去改会静默改不到（接口按 id 找活动）；
//   ④ 改名后重新拉一次 v1 列表，界面显示服务端的结果，而不是本地猜的结果。

import { calledChannels, installFakeBackend } from '@renderer/test/page-setup'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { RecordingTimeline } from './components/recording-timeline'

const activity = (id: number, title: string) => ({
  id: String(id),
  start_time: '2026-09-30 09:00:00',
  end_time: '2026-09-30 09:20:00',
  title,
  content: '',
  resources: []
})

const v1 = (legacyId: number, origin: unknown, title: string, id = `act-${legacyId}`) => ({
  id,
  legacy_id: legacyId,
  start: '2026-09-30T09:00:00Z',
  end: '2026-09-30T09:20:00Z',
  title,
  original_title: title,
  category: null,
  origin,
  confidence: 0.8,
  evidence: [],
  is_user_modified: false,
  derived_from_seq: 1
})

function renderTimeline(handlers: Record<string, unknown>) {
  const backend = installFakeBackend(handlers, { strict: false })
  render(
    <RecordingTimeline
      isMonitoring={false}
      isToday={false}
      canRecord={false}
      activities={[activity(11, '写导入脚本'), activity(12, '需求评审'), activity(13, '终端调试')]}
      recordingStats={null}
    />
  )
  return backend
}

describe('时间线：结论来源与改名（rust 后端）', () => {
  it('按来源渲染三种标记，推测带上模型名', async () => {
    renderTimeline({
      'v1:activities': {
        activities: [
          v1(11, { kind: 'inferred', model: 'openai_compatible:qwen3-vl' }, '写导入脚本'),
          v1(12, { kind: 'rule', rule_id: 'coding' }, '需求评审'),
          v1(13, { kind: 'observed' }, '终端调试')
        ]
      }
    })

    const inferred = await screen.findByTestId('provenance-11')
    expect(inferred).toHaveTextContent('推测')
    // 置信度要看得见：判断「这条结论值不值得信」靠的就是它
    expect(inferred).toHaveTextContent('80%')
    expect(inferred).toHaveAttribute('title', expect.stringContaining('qwen3-vl'))

    expect(screen.getByTestId('provenance-12')).toHaveTextContent('规则')
    expect(screen.getByTestId('provenance-13')).toHaveTextContent('观察到')
  })

  it('改名发的是 v1 的字符串 id，并在服务端确认后刷新标题', async () => {
    const backend = renderTimeline({
      'v1:activities': {
        activities: [v1(11, { kind: 'inferred', model: 'openai_compatible:qwen3-vl' }, '写导入脚本')]
      },
      'v1:activity-override': { ok: true }
    })

    const rename = await screen.findByTestId('rename-11')
    fireEvent.click(rename)
    const input = screen.getByLabelText('活动标题')
    fireEvent.change(input, { target: { value: '迁移旧库到新库' } })
    fireEvent.keyDown(input, { key: 'Enter', code: 'Enter' })

    await waitFor(() => expect(calledChannels(backend)).toContain('v1:activity-override'))
    const call = backend.calls.find((entry) => entry.channel === 'v1:activity-override')
    expect(call?.args).toEqual(['act-11', '迁移旧库到新库'])

    // 改完要重新拉一次列表：界面必须显示服务端算出来的结果
    await waitFor(() =>
      expect(backend.calls.filter((entry) => entry.channel === 'v1:activities').length).toBeGreaterThanOrEqual(2)
    )
  })

  it('合并修正：把某条并进时间上的上一条，发的是 v1 的字符串 id', async () => {
    const backend = renderTimeline({
      'v1:activities': {
        activities: [
          v1(11, { kind: 'inferred', model: 'openai_compatible:qwen3-vl' }, '写导入脚本'),
          v1(12, { kind: 'rule', rule_id: 'coding' }, '需求评审')
        ]
      },
      'v1:activity-merge': { ok: true }
    })

    // 列表倒序：11 在 12 之上，「并入上一条」把 11 并进 12
    fireEvent.click(await screen.findByTestId('merge-11'))

    await waitFor(() => expect(calledChannels(backend)).toContain('v1:activity-merge'))
    const call = backend.calls.find((entry) => entry.channel === 'v1:activity-merge')
    expect(call?.args).toEqual(['act-12', ['act-11']])

    await waitFor(() =>
      expect(backend.calls.filter((entry) => entry.channel === 'v1:activities').length).toBeGreaterThanOrEqual(2)
    )
  })

  it('切分修正：选时刻 + 后半段标题，发的是毫秒时间戳与 v1 字符串 id', async () => {
    const backend = renderTimeline({
      'v1:activities': {
        activities: [v1(11, { kind: 'inferred', model: 'openai_compatible:qwen3-vl' }, '写导入脚本')]
      },
      'v1:activity-split': { ok: true }
    })

    fireEvent.click(await screen.findByTestId('split-11'))
    fireEvent.change(screen.getByLabelText('拆分时刻'), { target: { value: '09:30' } })
    fireEvent.change(screen.getByLabelText('后半段标题'), { target: { value: '写导入脚本（续）' } })
    fireEvent.click(screen.getByTestId('split-confirm-11'))

    await waitFor(() => expect(calledChannels(backend)).toContain('v1:activity-split'))
    const call = backend.calls.find((entry) => entry.channel === 'v1:activity-split')
    const [activityId, atMs, tailTitle] = (call?.args ?? []) as [string, number, string]
    expect(activityId).toBe('act-11')
    expect(tailTitle).toBe('写导入脚本（续）')
    // 09:30 落在活动开始那天（2026-09-30）的本地时区
    const expected = new Date('2026-09-30T09:30:00')
    expect(atMs).toBe(expected.getTime())

    await waitFor(() =>
      expect(backend.calls.filter((entry) => entry.channel === 'v1:activities').length).toBeGreaterThanOrEqual(2)
    )
  })
})
