/** 设置页 / 空态：何时说明「录制尚未开始」（间隔滑块 ≠ 正在采集）。 */
export function shouldShowRecordingNotStarted(isMonitoring: boolean, enabled: boolean | undefined): boolean {
  if (isMonitoring) return false
  // enabled 未知时仍提示未开始：停止态下间隔配置容易被误读成「每 5s 在采」。
  if (enabled === undefined) return true
  return !enabled
}

/** 窗口列表空/权限：后端 `windows_reason` → 设置页是否弹权限 Alert。 */
export function isWindowListPermissionBlocked(windowsReason: string | null | undefined): boolean {
  return windowsReason === 'screen_recording_permission'
}

/** 设置页状态条：录制环 / capture.enabled 的可读态。 */
export type CaptureRecordingState = 'running' | 'enabled_idle' | 'stopped'

export function captureRecordingState(isMonitoring: boolean, enabled: boolean | undefined): CaptureRecordingState {
  if (isMonitoring) return 'running'
  if (enabled === true) return 'enabled_idle'
  return 'stopped'
}

/** 后端 `windows_reason` → 空列表原因（TCC vs 当前无打开窗口）。 */
export type WindowListHintKind = 'permission' | 'empty' | 'ok' | 'unknown'

export function windowListHintKind(windowsReason: string | null | undefined): WindowListHintKind {
  if (windowsReason === 'screen_recording_permission') return 'permission'
  if (windowsReason === 'empty') return 'empty'
  if (windowsReason === 'ok') return 'ok'
  return 'unknown'
}

/** 进程 TCC 明确为 false 时才展示「未授权」（未知时不误报）。 */
export function shouldShowTccDenied(tccGranted: boolean | undefined): boolean {
  return tccGranted === false
}

/** 改完屏幕录制开关后必须托盘完全退出再重开；仅关窗不够。 */
export function shouldShowQuitRelaunchHint(
  tccGranted: boolean | undefined,
  windowsReason: string | null | undefined
): boolean {
  return tccGranted === false || windowsReason === 'screen_recording_permission'
}
