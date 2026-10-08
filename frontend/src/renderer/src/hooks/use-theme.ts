// 跟随系统主题。
//
// 两套机制必须**一起**切，否则会出现「Arco 组件变了、自绘部分没变」的半暗半亮：
// - Arco 组件的暗色认 `body` 上的 `arco-theme="dark"`（样式在 `assets/theme/`）；
// - Tailwind 的 `dark:` 变体认 `documentElement` 上的 `dark` class。
//
// 另外 `main.css` 里有一组 `@media (prefers-color-scheme: dark)` 规则，
// 用来覆盖散落在各处的硬编码浅色（`bg-white` / `text-[var(--color-text-1)]` 等）——
// 那些原子类自己不会变色。

import { useEffect } from 'react'

const DARK_QUERY = '(prefers-color-scheme: dark)'

export function applyTheme(dark: boolean): void {
  document.documentElement.classList.toggle('dark', dark)
  document.body.setAttribute('arco-theme', dark ? 'dark' : '')
}

/** 跟随系统主题：挂载时应用一次，之后随系统切换实时更新。 */
export function useSystemTheme(): void {
  useEffect(() => {
    const query = window.matchMedia(DARK_QUERY)
    applyTheme(query.matches)

    const onChange = (event: MediaQueryListEvent): void => applyTheme(event.matches)
    query.addEventListener('change', onChange)
    return () => query.removeEventListener('change', onChange)
  }, [])
}
