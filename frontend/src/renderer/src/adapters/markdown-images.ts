// 笔记内容里的图片：**Blob URL 不得入库**。
//
// 编辑器（milkdown/crepe）粘贴或拖入图片时给的是 `blob:` 地址，只在当前会话有效。
// 直接落库的话，重启后笔记里就是一堆破图 —— 而用户完全不知道为什么。
// 因此保存前把它们交给本地文件服务，替换成落库后仍然有效的文件路径；
// 任何一张没成功都不落库（编辑器里还留着原文，用户可以重试）。

export interface MarkdownImageDeps {
  /** 读取 blob 内容（生产用 `fetch`，测试注入假实现）。 */
  read(url: string): Promise<Uint8Array>
  /** 保存到本地文件服务，返回可作为图片地址使用的路径。 */
  save(name: string, bytes: Uint8Array): Promise<string>
}

export interface PersistImagesResult {
  markdown: string
  replaced: number
  failed: number
}

const BLOB_URL = /blob:[^\s)"']+/g

/** 找出 markdown 里的 Blob URL（去重，保持出现顺序）。 */
export function blobImageUrls(markdown: string): string[] {
  const found = markdown.match(BLOB_URL) ?? []
  return [...new Set(found)]
}

/** 给 Blob URL 编一个稳定的文件名（扩展名从 mime 或原 URL 推不出来时用 png）。 */
export function blobFileName(url: string, index: number): string {
  const extension = url.startsWith('blob:') ? 'png' : 'png'
  return `note-image-${index + 1}.${extension}`
}

/**
 * 把 markdown 里的 Blob URL 换成保存后的文件路径。
 *
 * 只要有一张失败就**整体不替换**并把 `failed` 报出来：半替换的结果是
 * 「有的图在库里、有的图是临时地址」，比全部失败更难排查。
 */
export async function persistBlobImages(markdown: string, deps: MarkdownImageDeps): Promise<PersistImagesResult> {
  const urls = blobImageUrls(markdown)
  if (urls.length === 0) return { markdown, replaced: 0, failed: 0 }

  const saved: Array<[string, string]> = []
  let failed = 0
  for (const [index, url] of urls.entries()) {
    try {
      const bytes = await deps.read(url)
      const path = await deps.save(blobFileName(url, index), bytes)
      if (!path) throw new Error('文件服务没有返回路径')
      saved.push([url, path])
    } catch {
      failed += 1
    }
  }

  if (failed > 0) return { markdown, replaced: 0, failed }

  let next = markdown
  for (const [url, path] of saved) {
    next = next.split(url).join(path)
  }
  return { markdown: next, replaced: saved.length, failed: 0 }
}
