/**
 * 写入系统剪贴板。
 *
 * 优先级：
 * 1. Tauri 原生命令（WKWebView 里 `navigator.clipboard` 常因权限失败）
 * 2. `navigator.clipboard.writeText`
 * 3. `textarea` + `execCommand('copy')`（仍需在用户手势回调里调用）
 */

type TauriCore = {
  invoke?: (command: string, args?: Record<string, unknown>) => Promise<unknown>
}

function tauriInvoke(): TauriCore['invoke'] | undefined {
  if (typeof window === 'undefined') return undefined
  const core = (window as Window & { __TAURI__?: { core?: TauriCore } }).__TAURI__?.core
  return typeof core?.invoke === 'function' ? core.invoke.bind(core) : undefined
}

export async function writeClipboard(text: string): Promise<void> {
  const invoke = tauriInvoke()
  if (invoke) {
    try {
      await invoke('clipboard_write_text', { text })
      return
    } catch {
      // fall through to browser APIs
    }
  }

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
