// 外壳设置（开机自启）：统一入口。
//
// Tauri：`launch_at_login` 读系统实际值、`set_launch_at_login` 写完**再读回**；
// 外壳没有这条命令时返回 `none`，调用方据此**禁用控件并说明原因** ——
// 不做「点了没反应」的开关。

import type { TauriGlobals } from './tauri-shell.ts'

export type LaunchAtLoginShell = 'tauri' | 'none'

export interface LaunchAtLogin {
  shell: LaunchAtLoginShell
  /** 读系统实际值；外壳没有读取渠道时返回 undefined（界面应显示「未确认」）。 */
  read(): Promise<boolean | undefined>
  /** 写设置并尽量读回；返回 undefined 表示外壳不提供读回。 */
  write(enabled: boolean): Promise<boolean | undefined>
}

export function createLaunchAtLogin(target: unknown): LaunchAtLogin {
  const globals = (target ?? {}) as TauriGlobals
  const invoke = globals.__TAURI__?.core?.invoke

  if (typeof invoke === 'function') {
    return {
      shell: 'tauri',
      read: async () => Boolean(await invoke('launch_at_login')),
      write: async (enabled) => Boolean(await invoke('set_launch_at_login', { enabled }))
    }
  }

  return {
    shell: 'none',
    read: async () => undefined,
    write: async () => {
      throw new Error('开机自启需要桌面外壳提供（当前外壳未接线）')
    }
  }
}
