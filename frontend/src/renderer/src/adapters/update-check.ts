// 外壳更新检查：对照 GitHub Releases，返回新版本与发布页 / dmg 链接。
//
// 不做静默下载或安装（需要签名与公证）；`quitAndInstall` 仍由外壳桥明确失败。
// 没有外壳时返回 `none`，设置页据此禁用「检查更新」并说明原因。

import type { TauriGlobals } from './tauri-shell.ts'

export type UpdateCheckShell = 'tauri' | 'none'

export interface UpdateInfo {
  version: string
  htmlUrl: string
  dmgUrl?: string | null
}

export interface UpdateCheckResult {
  updateInfo: UpdateInfo | null
  currentVersion: string
}

export interface UpdateCheck {
  shell: UpdateCheckShell
  check(): Promise<UpdateCheckResult>
  openUrl(url: string): Promise<void>
}

export function createUpdateCheck(target: unknown): UpdateCheck {
  const globals = (target ?? {}) as TauriGlobals
  const invoke = globals.__TAURI__?.core?.invoke

  if (typeof invoke === 'function') {
    return {
      shell: 'tauri',
      check: async () => {
        const raw = (await invoke('check_for_update')) as {
          updateInfo?: UpdateInfo | null
          currentVersion?: string
        }
        return {
          updateInfo: raw?.updateInfo ?? null,
          currentVersion: raw?.currentVersion ?? ''
        }
      },
      openUrl: async (url) => {
        await invoke('open_external_url', { url })
      }
    }
  }

  return {
    shell: 'none',
    check: async () => {
      throw new Error('检查更新需要桌面外壳提供（当前外壳未接线）')
    },
    openUrl: async () => {
      throw new Error('打开更新链接需要桌面外壳提供（当前外壳未接线）')
    }
  }
}
