/**
 * 写入系统剪贴板。
 *
 * Tauri / 部分 WebView 里 `navigator.clipboard` 会因非安全上下文或权限拒绝而失败；
 * 失败时回退到 `textarea` + `execCommand('copy')`（仍需在用户手势回调里调用）。
 */
export async function writeClipboard(text: string): Promise<void> {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return
    } catch {
      // fall through
    }
  }

  const mount = typeof document !== 'undefined' ? document.body : null
  if (!mount) {
    throw new Error('clipboard unavailable')
  }

  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.left = '-9999px'
  area.style.top = '0'
  mount.appendChild(area)
  area.focus()
  area.select()
  let ok = false
  try {
    ok = document.execCommand('copy')
  } finally {
    mount.removeChild(area)
  }
  if (!ok) {
    throw new Error('clipboard unavailable')
  }
}
