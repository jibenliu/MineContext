// 外壳「导出诊断包」：统一入口。
//
// Tauri 命令 `export_diagnostics` 在 Downloads 写出脱敏 zip；
// 外壳没有这条命令时返回 `none`，界面禁用并说明原因。

import type { TauriGlobals } from './tauri-shell.ts'

export type ExportDiagnosticsShell = 'tauri' | 'none'

export interface ExportDiagnosticsResult {
  path: string
  folder: string
}

export interface ExportDiagnostics {
  shell: ExportDiagnosticsShell
  export(): Promise<ExportDiagnosticsResult>
}

export function createExportDiagnostics(target: unknown): ExportDiagnostics {
  const globals = (target ?? {}) as TauriGlobals
  const invoke = globals.__TAURI__?.core?.invoke

  if (typeof invoke === 'function') {
    return {
      shell: 'tauri',
      export: async () => {
        const result = (await invoke('export_diagnostics')) as ExportDiagnosticsResult
        if (!result?.path) {
          throw new Error('诊断导出未返回路径')
        }
        return result
      }
    }
  }

  return {
    shell: 'none',
    export: async () => {
      throw new Error('诊断导出需要桌面外壳提供（当前外壳未接线）')
    }
  }
}
