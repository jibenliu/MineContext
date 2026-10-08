// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { typeIconMap } from '@renderer/utils/file'
import { useCallback, useEffect, useState } from 'react'

export interface Document {
  name: string
  source: string
  status: string
  icon: string
  prompt: string
  filePath: string
}

export const useFiles = () => {
  const [analyzedDocs, setAnalyzedDocs] = useState<Document[]>([])
  const [loading, setLoading] = useState(false)
  const [loadFailed, setLoadFailed] = useState(false)

  const loadFiles = useCallback(async () => {
    setLoading(true)
    setLoadFailed(false)
    try {
      const result = await window.fileService.getFiles()
      if (!result.success || !result.files) throw new Error('Files unavailable')
      const filesWithIcons = result.files
        .filter((file) => !file.name.startsWith('.')) // Filter out hidden files starting with .
        .map((file) => ({
          ...file,
          icon: typeIconMap[file.name.split('.').pop()?.toLowerCase() ?? ''] ?? typeIconMap.txt
        }))
      setAnalyzedDocs(filesWithIcons)
    } catch {
      setLoadFailed(true)
    } finally {
      setLoading(false)
    }
  }, [])

  const saveFile = useCallback(async (fileName: string, fileData: Uint8Array) => {
    const result = await window.fileService.saveFile(fileName, fileData)
    return result
  }, [])

  useEffect(() => {
    loadFiles()
  }, [loadFiles])

  const addFile = useCallback((file: Document) => {
    setAnalyzedDocs((prev) => [...prev.filter((entry) => entry.name !== file.name), file])
  }, [])

  return { analyzedDocs, addFile, loadFiles, saveFile, loading, loadFailed }
}
