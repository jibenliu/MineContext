// 托盘状态文案：唯一真相在 flags，展示串走产品 i18n。
// 外壳只负责贴字，不在 Rust 里再维护一份业务文案。

import { type Locale, type MessageParams, translate } from '@renderer/i18n'

export type TrayStatusFlags = {
  recording: boolean
  indexingPaused: boolean
  daemonDisconnected: boolean
}

export type TrayStatusPresentation = {
  recording: boolean
  tooltip: string
  toggleLabel: string
  /** macOS 菜单栏图标旁短标题；空串表示不显示 */
  title: string
}

type Translate = (key: string, params?: MessageParams) => string

/** 从 flags 拼出托盘提示 / 菜单 / 短标题（优先级：断连 > 录制态 + 可选索引暂停）。 */
export function trayStatusPresentation(flags: TrayStatusFlags, t: Translate): TrayStatusPresentation {
  const toggleLabel = t(flags.recording ? 'screenMonitor.stopRecording' : 'screenMonitor.startRecording')

  if (flags.daemonDisconnected) {
    return {
      recording: flags.recording,
      tooltip: t('tray.tooltip.disconnected'),
      toggleLabel,
      title: t('tray.title.disconnected')
    }
  }

  const base = t(flags.recording ? 'tray.tooltip.recording' : 'tray.tooltip.idle')
  const tooltip = flags.indexingPaused ? `${base} · ${t('tray.tooltip.indexingPausedSuffix')}` : base
  const title = flags.indexingPaused
    ? t('tray.title.indexingPaused')
    : flags.recording
      ? t('tray.title.recording')
      : t('tray.title.idle')

  return { recording: flags.recording, tooltip, toggleLabel, title }
}

/** 非 React 路径（hook 内）用当前 store 语言拼装。 */
export function presentTrayStatus(flags: TrayStatusFlags, locale: Locale): TrayStatusPresentation {
  return trayStatusPresentation(flags, (key, params) => translate(locale, key, params))
}

/** health / init-check 里 embedding.status === 'paused' 即索引因鉴权暂停。 */
export function indexingPausedFromHealth(raw: unknown): boolean {
  if (!raw || typeof raw !== 'object') return false
  const data = (raw as { data?: unknown }).data
  const root = data && typeof data === 'object' ? data : raw
  const components = (root as { components?: unknown }).components
  if (!components || typeof components !== 'object') return false
  const embedding = (components as { embedding?: unknown }).embedding
  if (!embedding || typeof embedding !== 'object') return false
  return (embedding as { status?: unknown }).status === 'paused'
}
