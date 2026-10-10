import { describe, expect, it } from 'vitest'

import {
  captureRecordingState,
  isWindowListPermissionBlocked,
  shouldShowQuitRelaunchHint,
  shouldShowRecordingNotStarted,
  shouldShowTccDenied,
  windowListHintKind
} from './recording-status-copy'

describe('shouldShowRecordingNotStarted', () => {
  it('录制中不提示', () => {
    expect(shouldShowRecordingNotStarted(true, false)).toBe(false)
    expect(shouldShowRecordingNotStarted(true, true)).toBe(false)
  })

  it('已停止且 enabled=false 时提示未开始', () => {
    expect(shouldShowRecordingNotStarted(false, false)).toBe(true)
  })

  it('已停止但 enabled=true（尚未点开始 / 将自动开录）时不误报卡住', () => {
    expect(shouldShowRecordingNotStarted(false, true)).toBe(false)
  })

  it('enabled 未知时按未开始提示，避免只看见 5s 间隔', () => {
    expect(shouldShowRecordingNotStarted(false, undefined)).toBe(true)
  })
})

describe('isWindowListPermissionBlocked', () => {
  it('只把 TCC 原因当成权限拦截', () => {
    expect(isWindowListPermissionBlocked('screen_recording_permission')).toBe(true)
    expect(isWindowListPermissionBlocked('empty')).toBe(false)
    expect(isWindowListPermissionBlocked('ok')).toBe(false)
    expect(isWindowListPermissionBlocked(null)).toBe(false)
  })
})

describe('captureRecordingState', () => {
  it('区分正在录 / 已启用未开录 / 已停止', () => {
    expect(captureRecordingState(true, true)).toBe('running')
    expect(captureRecordingState(false, true)).toBe('enabled_idle')
    expect(captureRecordingState(false, false)).toBe('stopped')
    expect(captureRecordingState(false, undefined)).toBe('stopped')
  })
})

describe('windowListHintKind', () => {
  it('区分 TCC 拦截与当前无打开窗口', () => {
    expect(windowListHintKind('screen_recording_permission')).toBe('permission')
    expect(windowListHintKind('empty')).toBe('empty')
    expect(windowListHintKind('ok')).toBe('ok')
    expect(windowListHintKind(null)).toBe('unknown')
    expect(windowListHintKind(undefined)).toBe('unknown')
  })
})

describe('shouldShowTccDenied', () => {
  it('仅在明确 false 时提示 TCC 未授权', () => {
    expect(shouldShowTccDenied(false)).toBe(true)
    expect(shouldShowTccDenied(true)).toBe(false)
    expect(shouldShowTccDenied(undefined)).toBe(false)
  })
})

describe('shouldShowQuitRelaunchHint', () => {
  it('TCC 未授权或窗口权限原因时提示完全退出再重开', () => {
    expect(shouldShowQuitRelaunchHint(false, null)).toBe(true)
    expect(shouldShowQuitRelaunchHint(true, 'screen_recording_permission')).toBe(true)
    expect(shouldShowQuitRelaunchHint(true, 'empty')).toBe(false)
    expect(shouldShowQuitRelaunchHint(true, 'ok')).toBe(false)
    expect(shouldShowQuitRelaunchHint(undefined, null)).toBe(false)
  })
})
