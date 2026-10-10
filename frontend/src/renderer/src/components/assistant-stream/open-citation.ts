// 把引用路径交给导航钩子：vault 走 setActiveVault，其它走主 tab。

import type { CitationSource } from './citation-target'
import { citationPath } from './citation-target'

export interface CitationNavigator {
  navigateToVault: (vaultId: number) => void
  navigateToMainTab: (tabKey: string, path: string) => void
}

export function openCitation(source: CitationSource, nav: CitationNavigator): boolean {
  const path = citationPath(source)
  if (!path) return false
  if (path.startsWith('/vault')) {
    const id = Number(new URL(path, 'http://local').searchParams.get('id'))
    if (!Number.isFinite(id)) return false
    nav.navigateToVault(id)
    return true
  }
  if (path.startsWith('/screen-monitor')) {
    nav.navigateToMainTab('screen-monitor', path)
    return true
  }
  if (path.startsWith('/summaries')) {
    nav.navigateToMainTab('summaries', path)
    return true
  }
  return false
}
