// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { CaptureSource } from '@interface/common/source'

/** 从窗口标题启发式抽出应用名（仅在后端未给 appName 时使用）。 */
function appNameFromTitle(sourceName: string): string {
  let appName = sourceName

  // Microsoft Teams specific patterns
  if (
    sourceName.includes('Microsoft Teams') ||
    sourceName.includes('MSTeams') ||
    (sourceName.includes('Chat |') && sourceName.includes('| Microsoft Teams'))
  ) {
    return 'Microsoft Teams'
  }
  // WeChat specific patterns
  if (sourceName.includes('WeChat') || sourceName.includes('微信')) {
    return 'WeChat'
  }
  // Slack specific patterns
  if (sourceName.includes('Slack')) {
    return 'Slack'
  }
  // Chrome specific patterns
  if (sourceName.includes('Google Chrome') || sourceName.endsWith(' - Chrome')) {
    return 'Google Chrome'
  }
  // Safari specific patterns
  if (sourceName.includes('Safari') || sourceName.endsWith(' — Safari')) {
    return 'Safari'
  }
  // Visual Studio Code
  if (sourceName.includes('Visual Studio Code') || sourceName.endsWith(' - Code')) {
    return 'Visual Studio Code'
  }
  // Terminal/iTerm
  if (sourceName.includes('Terminal') || sourceName.includes('iTerm')) {
    return sourceName.includes('iTerm') ? 'iTerm' : 'Terminal'
  }
  // For other apps, try to extract from window title more carefully
  if (sourceName.includes(' — ')) {
    // For apps that use em dash separator (like many Mac apps)
    // Take the last part, but only if it looks like an app name (not too long)
    const lastPart = sourceName.split(' — ').pop()
    if (lastPart && lastPart.length < 30) {
      appName = lastPart
    }
  } else if (sourceName.includes(' - ')) {
    // For apps that use regular dash separator
    // Be more careful - only take the last part if it's likely an app name
    const parts = sourceName.split(' - ')
    const lastPart = parts[parts.length - 1]

    // Check if the last part looks like an app name (starts with capital, not too long, etc.)
    if (
      lastPart &&
      lastPart.length < 30 &&
      /^[A-Z]/.test(lastPart) &&
      !lastPart.includes('.') && // Not a filename
      !lastPart.includes('/') && // Not a path
      !lastPart.match(/^\d/)
    ) {
      // Doesn't start with a number
      appName = lastPart
    }
  }

  // Final cleanup - if appName is still the full window title and it's very long,
  // just use the first part before any separator
  if (appName === sourceName && appName.length > 50) {
    const firstPart = appName.split(/[-—]/)[0].trim()
    if (firstPart && firstPart.length < 30) {
      appName = firstPart
    }
  }

  return appName
}

const formatName = (sources?: CaptureSource[]) => {
  const appGroups = new Map<string, CaptureSource>()

  ;(sources || []).forEach((source) => {
    if (source.type === 'screen') {
      // Keep all screens as-is
      appGroups.set(source.id, source)
      return
    }

    // 优先用后端 appName：标题启发式会把「Menu Bar」等系统窗和真应用搅在一起，
    // 也会让同名标题在 Checkbox 里只剩一个可勾选项。
    const backendApp = source.appName?.trim()
    const appName = backendApp || appNameFromTitle(source.name)

    // If we already have this app, prefer the main window over sub-windows
    const existingSource = appGroups.get(appName)

    if (!existingSource) {
      // 列表展示用应用名，避免两个窗口都显示成「Menu Bar」标题
      appGroups.set(appName, { ...source, appName, name: appName })
    } else {
      // Special handling for Microsoft Teams windows（用标题判定，name 已被改成应用名）
      if (appName === 'Microsoft Teams') {
        const currentTitle = source.windowTitle || source.name
        const existingTitle = existingSource.windowTitle || existingSource.name
        const isCurrentMSTeams = currentTitle.includes('MSTeams')
        const isExistingMSTeams = existingTitle.includes('MSTeams')
        const isCurrentChat = currentTitle.includes('Chat |')
        const isExistingChat = existingTitle.includes('Chat |')

        if (isCurrentMSTeams && !isExistingMSTeams) {
          // Current is MSTeams main window, prefer it over chat windows
          appGroups.set(appName, { ...source, appName, name: appName })
        } else if (!isCurrentMSTeams && isExistingMSTeams) {
          // Existing is MSTeams main window, keep it
        } else if (isCurrentChat && !isExistingChat) {
          // Current is chat, existing is something else - prefer chat over generic
          appGroups.set(appName, { ...source, appName, name: appName })
        } else {
          // Default: prefer shorter window title
          if (currentTitle.length < existingTitle.length) {
            appGroups.set(appName, { ...source, appName, name: appName })
          }
        }
      } else {
        // Original logic for non-Teams apps — compare against window title when present
        const currentTitle = source.windowTitle || source.name
        const existingTitle = existingSource.windowTitle || existingSource.name
        const isCurrentMainWindow = currentTitle === appName || currentTitle.endsWith(appName)
        const isExistingMainWindow = existingTitle === appName || existingTitle.endsWith(appName)

        if (isCurrentMainWindow && !isExistingMainWindow) {
          // Current is main window, existing is sub-window - replace
          appGroups.set(appName, { ...source, appName, name: appName })
        } else if (!isCurrentMainWindow && !isExistingMainWindow) {
          // Both are sub-windows - prefer shorter name (usually more general)
          if (currentTitle.length < existingTitle.length) {
            appGroups.set(appName, { ...source, appName, name: appName })
          }
        }
      }
    }
  })

  const result = Array.from(appGroups.values())
  return result
}
export { formatName }
