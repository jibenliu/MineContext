// `window.fileService` 的形状。

import type { Backend } from './types.ts'

export interface FileImportResult {
  id: number
  title: string
  name: string
  kind: string
  file_path: string
}

export interface FileService {
  saveFile(fileName: string, fileData: Uint8Array | string): Promise<unknown>
  /** P1：抽取正文写入笔记树，同时保留 uploads 原文件。 */
  importFile(fileName: string, fileData: Uint8Array | string, parentId?: number | null): Promise<FileImportResult>
  readFile(filePath: string, encoding?: 'base64'): Promise<unknown>
  copyFile(srcPath: string): Promise<unknown>
  getFiles(): Promise<unknown>
}

export function createFileService(backend: Backend): FileService {
  return {
    saveFile: (fileName, fileData) => backend.invoke('file:save', fileName, fileData),
    importFile: (fileName, fileData, parentId) =>
      backend.invoke('v1:import-file', fileName, fileData, parentId ?? null) as Promise<FileImportResult>,
    readFile: (filePath, encoding) =>
      encoding ? backend.invoke('file:read', filePath, encoding) : backend.invoke('file:read', filePath),
    copyFile: (srcPath) => backend.invoke('file:copy', srcPath),
    getFiles: () => backend.invoke('file:get-all')
  }
}
