// index.html 里的启动占位（#spinner）不会自己消失：React 挂载后必须显式移除，
// 否则它会一直盖在窗口中央。
export const STARTUP_SPINNER_ID = 'spinner'

export function removeStartupSpinner(doc: Document = document): boolean {
  const spinner = doc.getElementById(STARTUP_SPINNER_ID)
  if (!spinner) return false
  spinner.remove()
  return true
}
