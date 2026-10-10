import { describe, expect, it } from 'vitest'

import { isCapturePlatformSupported } from '@renderer/pages/screen-monitor/capture-platform'

import { isScreenRecordingGranted } from './use-screen'

describe('isScreenRecordingGranted', () => {
  it('认后端 permissions 结构体的 screen_recording', () => {
    expect(isScreenRecordingGranted({ screen_recording: true, permission: 'granted' })).toBe(true)
    expect(isScreenRecordingGranted({ screen_recording: false, permission: 'denied' })).toBe(false)
  })

  it('认 permission / status 标签与旧布尔', () => {
    expect(isScreenRecordingGranted({ permission: 'granted' })).toBe(true)
    expect(isScreenRecordingGranted({ permission: 'not_required' })).toBe(true)
    expect(isScreenRecordingGranted({ status: 'granted' })).toBe(true)
    expect(isScreenRecordingGranted(true)).toBe(true)
    expect(isScreenRecordingGranted(false)).toBe(false)
    expect(isScreenRecordingGranted(null)).toBe(false)
  })
})

describe('isCapturePlatformSupported（与权限正交）', () => {
  it('capture_supported=false 时即使 permission 字段存在也不当「可授权」', () => {
    expect(
      isCapturePlatformSupported({
        capture_supported: false,
        screen_recording: false,
        permission: 'unknown'
      })
    ).toBe(false)
  })
})
