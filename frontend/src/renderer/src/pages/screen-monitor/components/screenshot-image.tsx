// 截图缩略图：经受鉴权的适配层读成 base64 再显示（`file://` 在外壳 webview 里取不到），
// 并带两件「取不到图」时的兜底：可重试的失败态，以及右键删除。
//
// 删除不可逆（文件与库里的引用一起删），所以入口放在右键菜单里 —— 需要显式一步。

import { Dropdown, Image, Menu, Message } from '@arco-design/web-react'
import { useI18n } from '@renderer/i18n'
import { useEffect, useState } from 'react'

export function ScreenshotImage({
  path,
  alt,
  index,
  onDeleted
}: {
  path: string
  alt: string
  index: number
  /** 删除成功后通知调用方（列表要重取时用；不给则只在本组件内隐藏） */
  onDeleted?: (path: string) => void
}) {
  const { t } = useI18n()
  const [attempt, setAttempt] = useState(0)
  const [deleted, setDeleted] = useState(false)
  const [result, setResult] = useState<{ path: string; attempt: number; src?: string }>()

  useEffect(() => {
    let active = true
    const load = async () => {
      try {
        const image = await window.screenMonitorAPI.readImageAsBase64(path)
        if (image.success === false || !image.data) throw new Error('Image unavailable')
        const mime = image.mime === 'image/jpeg' ? 'image/jpeg' : 'image/png'
        if (active) setResult({ path, attempt, src: `data:${mime};base64,${image.data}` })
      } catch {
        if (active) setResult({ path, attempt })
      }
    }
    void load()
    return () => {
      active = false
    }
  }, [path, attempt])

  const remove = async (): Promise<void> => {
    try {
      await window.screenMonitorAPI.deleteScreenshot(path)
      setDeleted(true)
      onDeleted?.(path)
    } catch {
      Message.error(t('screenMonitor.screenshot.deleteFailed'))
    }
  }

  // 已删除：不占位，列表自行重排
  if (deleted) return null

  const current = result?.path === path && result.attempt === attempt ? result : undefined

  const menu = (
    <Menu className="w-[160px] text-[12px]" onClickMenuItem={() => void remove()}>
      <Menu.Item key="delete">{t('screenMonitor.screenshot.delete')}</Menu.Item>
    </Menu>
  )

  let thumbnail = (
    <div aria-busy="true" aria-label={alt} className="h-[60px] w-[110px] rounded-[8px] bg-[var(--color-fill-2)]" />
  )
  if (current && !current.src) {
    thumbnail = (
      <button
        type="button"
        title={t('common.loadFailed')}
        className="h-[60px] w-[110px] rounded-[8px] bg-[var(--color-fill-2)] text-xs"
        onClick={() => setAttempt((previous) => previous + 1)}>
        {t('common.loadFailed')} · {t('common.retry')}
      </button>
    )
  } else if (current?.src) {
    thumbnail = (
      <Image
        src={current.src}
        width={110}
        height={60}
        alt={alt}
        index={index}
        className="cursor-pointer rounded-[8px] overflow-hidden [&_img]:object-cover"
        onError={() => setResult({ path, attempt })}
      />
    )
  }

  return (
    <Dropdown trigger="contextMenu" droplist={menu}>
      {thumbnail}
    </Dropdown>
  )
}
