import { getLocale, translate } from '@renderer/i18n'

const TYPES: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp'
}
const MAX_IMAGE_BYTES = 1024 * 1024

/** 笔记图片落库契约：markdown 里一律写 `file://` 绝对路径，重启后由 `resolveEditorImage` 读回。 */
export function toEditorFileUrl(absolutePath: string): string {
  const address = new URL('file:///')
  address.pathname = absolutePath
  return address.href
}

export async function persistEditorImage(file: File): Promise<string> {
  const extension = TYPES[file.type]
  if (!extension || file.size > MAX_IMAGE_BYTES) {
    throw new Error(translate(getLocale(), 'editor.imageLimit'))
  }
  const encoded = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).split(',')[1])
    reader.onerror = () => reject(reader.error)
    reader.onabort = () => reject(new Error(translate(getLocale(), 'editor.imageFailed')))
    reader.readAsDataURL(file)
  })
  const result = await window.fileService.saveFile(`note-image-${crypto.randomUUID()}.${extension}`, encoded)
  if (!result?.success || typeof result.filePath !== 'string' || !result.filePath.startsWith('/')) {
    throw new Error(translate(getLocale(), 'editor.imageFailed'))
  }
  return toEditorFileUrl(result.filePath)
}

export async function resolveEditorImage(address: string): Promise<string> {
  if (address.startsWith('blob:')) throw new Error(translate(getLocale(), 'editor.imageExpired'))
  // 历史笔记可能存了裸绝对路径；与 `file://` 走同一读回路径，避免重启破图。
  const fileUrl = address.startsWith('file:') ? address : address.startsWith('/') ? toEditorFileUrl(address) : null
  if (!fileUrl) return address
  const url = new URL(fileUrl)
  const name = decodeURIComponent(url.pathname.split('/').pop() ?? '')
  const extension = name.split('.').pop()?.toLowerCase()
  const type = Object.entries(TYPES).find(([, value]) => value === extension)?.[0]
  if (!type) throw new Error(translate(getLocale(), 'editor.imageLimit'))
  const result = await window.fileService.readFile(name, 'base64')
  if (!result?.success || typeof result.data !== 'string') {
    throw new Error(translate(getLocale(), 'editor.imageFailed'))
  }
  return `data:${type};base64,${result.data}`
}
