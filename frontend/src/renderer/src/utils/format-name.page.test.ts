import type { CaptureSource } from '@interface/common/source'
import { describe, expect, it } from 'vitest'

import { formatName } from './format-name'

function windowSource(partial: Partial<CaptureSource> & Pick<CaptureSource, 'id' | 'name'>): CaptureSource {
  return {
    type: 'window',
    thumbnail: null,
    appIcon: null,
    isVisible: true,
    ...partial
  }
}

describe('formatName', () => {
  it('prefers backend appName over title heuristics', () => {
    const sources = [
      windowSource({
        id: 'window-1',
        name: 'main.rs — VSCode',
        appName: 'Visual Studio Code',
        windowTitle: 'main.rs — VSCode'
      }),
      windowSource({
        id: 'window-2',
        name: 'README.md — VSCode',
        appName: 'Visual Studio Code',
        windowTitle: 'README.md — VSCode'
      })
    ]

    const grouped = formatName(sources)
    expect(grouped).toHaveLength(1)
    expect(grouped[0].appName).toBe('Visual Studio Code')
    expect(grouped[0].name).toBe('Visual Studio Code')
  })

  it('keeps distinct apps that share a Menu Bar style title when appName differs', () => {
    const sources = [
      windowSource({
        id: 'window-a',
        name: 'Menu Bar',
        appName: 'Control Center',
        windowTitle: 'Menu Bar'
      }),
      windowSource({
        id: 'window-b',
        name: 'Menu Bar',
        appName: 'SystemUIServer',
        windowTitle: 'Menu Bar'
      })
    ]

    const grouped = formatName(sources)
    expect(grouped.map((s) => s.appName).sort()).toEqual(['Control Center', 'SystemUIServer'])
    expect(new Set(grouped.map((s) => s.id)).size).toBe(2)
  })

  it('leaves screens untouched', () => {
    const screens = [
      {
        id: 'display-1',
        name: 'Built-in',
        type: 'screen' as const,
        thumbnail: 'data:image/png;base64,abc',
        appIcon: null,
        isVisible: true
      }
    ]
    expect(formatName(screens)).toEqual(screens)
  })
})
