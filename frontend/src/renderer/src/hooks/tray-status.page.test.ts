import { describe, expect, it } from 'vitest'

import { indexingPausedFromHealth, type TrayStatusFlags, trayStatusPresentation } from './tray-status'

const zh: Record<string, string> = {
  'screenMonitor.startRecording': '开始录制',
  'screenMonitor.stopRecording': '停止录制',
  'tray.tooltip.recording': 'MineContext · 录制中',
  'tray.tooltip.idle': 'MineContext · 已暂停',
  'tray.tooltip.indexingPausedSuffix': '索引已暂停',
  'tray.tooltip.disconnected': 'MineContext · 无法连接本地服务',
  'tray.title.recording': '录',
  'tray.title.idle': '',
  'tray.title.indexingPaused': '停',
  'tray.title.disconnected': '断'
}

const t = (key: string) => zh[key] ?? key

function present(flags: TrayStatusFlags) {
  return trayStatusPresentation(flags, t)
}

describe('trayStatusPresentation', () => {
  it('录制中：提示与菜单跟产品词条一致', () => {
    expect(present({ recording: true, indexingPaused: false, daemonDisconnected: false })).toEqual({
      recording: true,
      tooltip: 'MineContext · 录制中',
      toggleLabel: '停止录制',
      title: '录'
    })
  })

  it('未录制：提示为已暂停，菜单为开始录制', () => {
    expect(present({ recording: false, indexingPaused: false, daemonDisconnected: false })).toEqual({
      recording: false,
      tooltip: 'MineContext · 已暂停',
      toggleLabel: '开始录制',
      title: ''
    })
  })

  it('索引暂停叠在录制态上，短标题优先显示暂停', () => {
    const out = present({ recording: true, indexingPaused: true, daemonDisconnected: false })
    expect(out.tooltip).toBe('MineContext · 录制中 · 索引已暂停')
    expect(out.title).toBe('停')
    expect(out.toggleLabel).toBe('停止录制')
  })

  it('daemon 断连压过录制与索引态', () => {
    const out = present({ recording: true, indexingPaused: true, daemonDisconnected: true })
    expect(out.tooltip).toBe('MineContext · 无法连接本地服务')
    expect(out.title).toBe('断')
  })
})

describe('indexingPausedFromHealth', () => {
  it('识别 health / init-check 的 embedding.paused', () => {
    expect(
      indexingPausedFromHealth({
        data: { components: { embedding: { status: 'paused' } } }
      })
    ).toBe(true)
    expect(
      indexingPausedFromHealth({
        data: { components: { embedding: { status: 'ok' } } }
      })
    ).toBe(false)
  })
})
