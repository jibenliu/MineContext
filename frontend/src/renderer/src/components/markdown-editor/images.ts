import { getLocale, translate } from '@renderer/i18n'

const TYPES: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp'
}
const MAX_IMAGE_BYTES = 1024 * 1024

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
  const address = new URL('file:///')
  address.pathname = result.filePath
  return address.href
}

export async function resolveEditorImage(address: string): Promise<string> {
  if (address.startsWith('blob:')) throw new Error(translate(getLocale(), 'editor.imageExpired'))
  if (!address.startsWith('file:')) return address
  const url = new URL(address)
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
