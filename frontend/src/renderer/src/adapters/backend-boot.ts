// 桌面外壳负责拉起 daemon：正常启动路径不应把「还在等 runtime」显示成硬错误。
//
// `backendReady === false` 只表示首次 bootstrap 未读到 runtime；外壳可能仍在写
// `runtime.json`，或 `get_runtime` 需要补读磁盘。等待中用 starting，真正用尽
// 重试后再升为 error。

export type BackendBootPhase = 'ready' | 'waiting' | 'failed'

/** Loading 文案/重试按钮的契约：waiting 不得出现「无法连接本地服务」。 */
export function loadingStatusForBootPhase(phase: BackendBootPhase): 'starting' | 'running' | 'error' {
  switch (phase) {
    case 'ready':
      return 'running'
    case 'waiting':
      return 'starting'
    case 'failed':
      return 'error'
  }
}

export function shouldShowBackendUnavailable(phase: BackendBootPhase): boolean {
  return phase === 'failed'
}

/** 硬错误文案承诺了「从设置继续配置」——UI 必须给出出口，不能只剩重试。 */
export function shouldOfferSettingsEscape(phase: BackendBootPhase): boolean {
  return phase === 'failed'
}
