import { describe, expect, it } from 'vitest'

import { isWindowListPermissionBlocked, shouldShowRecordingNotStarted } from './recording-status-copy'

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
