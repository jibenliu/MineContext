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
