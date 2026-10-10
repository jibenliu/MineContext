/** 后端 permissions / status 的 `capture_supported`：是否有屏幕/窗口采集实现。 */

export function isCapturePlatformSupported(result: unknown): boolean {
  if (!result || typeof result !== 'object') return true
  const body = result as Record<string, unknown>
  if (typeof body.capture_supported === 'boolean') return body.capture_supported
  // 旧 daemon 无此字段时按受支持处理，避免误伤 macOS 旧包。
  return true
}

export type ScreenMonitorGate = 'unsupported' | 'permission' | 'ready'

/** 屏幕监控页主门闩：平台未实现优先于权限 CTA。 */
export function screenMonitorGate(input: {
  captureSupported: boolean
  hasPermission: boolean
}): ScreenMonitorGate {
  if (!input.captureSupported) return 'unsupported'
  if (!input.hasPermission) return 'permission'
  return 'ready'
}
