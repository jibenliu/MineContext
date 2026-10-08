// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

// 图片地址约束：Blob URL 只在当前会话有效，**不得入库**；落库的必须是本地文件 URL。
import '@milkdown/crepe/theme/common/style.css'
import '@milkdown/crepe/theme/frame.css'
import './index.css'

import { Crepe } from '@milkdown/crepe'
import { Milkdown, MilkdownProvider, useEditor } from '@milkdown/react'
import { useRef } from 'react'

import { persistEditorImage, resolveEditorImage } from './images'

const CrepeEditor: React.FC<{ defaultValue: string; onChange: (value: string) => void }> = ({
  defaultValue,
  onChange
}) => {
  // `useEditor` 只在挂载时执行一次，闭包里捕获的 `onChange` 会是首次渲染的那个。
  // 父组件重建回调（例如保存函数依赖了文档 id）后，编辑器仍会调用旧回调 ——
  // 表现为"同一文档内改动存到了旧 id"。用 ref 取最新回调，不再依赖父组件的 key 重建。
  const onChangeRef = useRef(onChange)
  onChangeRef.current = onChange

  useEditor((root) => {
    const crepe = new Crepe({
      root,
      defaultValue,
      featureConfigs: {
        [Crepe.Feature.ImageBlock]: { onUpload: persistEditorImage, proxyDomURL: resolveEditorImage }
      }
    })
    // Listen for content changes
    crepe.on((listener) => {
      listener.markdownUpdated((_, markdown) => {
        onChangeRef.current(markdown)
      })
    })
    return crepe
  })

  return <Milkdown />
}

const MarkdownEditor = (props: { defaultValue: string; onChange: (value: string) => void }) => {
  return (
    <MilkdownProvider>
      <CrepeEditor {...props} />
    </MilkdownProvider>
  )
}

export default MarkdownEditor
