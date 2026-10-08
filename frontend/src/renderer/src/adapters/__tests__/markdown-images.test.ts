// 笔记里的图片：Blob URL 不得入库（只在本会话有效，重启就是破图）。
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { blobImageUrls, persistBlobImages } from '../markdown-images.ts'

const markdown = '正文\n\n![图](blob:http://localhost/abc-123)\n\n结尾'
const bytes = new Uint8Array([1, 2, 3])

test('识别 Blob URL（去重、保持顺序）', () => {
  assert.deepEqual(blobImageUrls(markdown), ['blob:http://localhost/abc-123'])
  assert.deepEqual(blobImageUrls('![a](blob:x) ![b](blob:x)'), ['blob:x'])
  assert.deepEqual(blobImageUrls('没有图片'), [])
})

test('全部上传成功：替换成落库后仍有效的路径', async () => {
  const saved: string[] = []
  const result = await persistBlobImages(markdown, {
    read: async () => bytes,
    save: async (name, data) => {
      saved.push(`${name}:${data.length}`)
      return '/uploads/note-image-1.png'
    }
  })

  assert.equal(result.failed, 0)
  assert.equal(result.replaced, 1)
  assert.ok(!result.markdown.includes('blob:'), '不许留下 Blob URL')
  assert.ok(result.markdown.includes('/uploads/note-image-1.png'))
  assert.deepEqual(saved, ['note-image-1.png:3'])
})

test('任何一张失败就整体不替换（半替换的结果更难排查）', async () => {
  const result = await persistBlobImages(markdown, {
    read: async () => bytes,
    save: async () => {
      throw new Error('文件服务不可用')
    }
  })

  assert.equal(result.replaced, 0)
  assert.equal(result.failed, 1)
  assert.equal(result.markdown, markdown, '失败时保持原文，编辑器里还能重试')
})

test('没有 Blob URL 时是零开销的空操作', async () => {
  let called = 0
  const result = await persistBlobImages('纯文字', {
    read: async () => bytes,
    save: async () => {
      called += 1
      return '/x'
    }
  })
  assert.deepEqual(result, { markdown: '纯文字', replaced: 0, failed: 0 })
  assert.equal(called, 0)
})
