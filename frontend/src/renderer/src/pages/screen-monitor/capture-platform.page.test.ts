// 业务测试：非 macOS 不得走「开启屏幕录制权限」路径。
// capture_supported=false 时界面应提示平台不支持，而不是假装 TCC 能修好。

import { describe, expect, it } from 'vitest'

import { isCapturePlatformSupported, screenMonitorGate } from './capture-platform'

describe('isCapturePlatformSupported', () => {
  it('认后端 capture_supported', () => {
    expect(isCapturePlatformSupported({ capture_supported: true })).toBe(true)
    expect(isCapturePlatformSupported({ capture_supported: false })).toBe(false)
  })

  it('缺字段时按受支持处理（旧 daemon 兼容）', () => {
    expect(isCapturePlatformSupported({ screen_recording: false })).toBe(true)
    expect(isCapturePlatformSupported(null)).toBe(true)
  })
})

describe('screenMonitorGate', () => {
  it('平台不支持时走 unsupported，不走 permission CTA', () => {
    expect(screenMonitorGate({ captureSupported: false, hasPermission: false })).toBe('unsupported')
    expect(screenMonitorGate({ captureSupported: false, hasPermission: true })).toBe('unsupported')
  })

  it('平台支持且无权限时走 permission', () => {
    expect(screenMonitorGate({ captureSupported: true, hasPermission: false })).toBe('permission')
  })

  it('平台支持且有权限时走 ready', () => {
    expect(screenMonitorGate({ captureSupported: true, hasPermission: true })).toBe('ready')
  })
})
