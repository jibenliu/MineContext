// `window.fileService` 的形状。

import type { Backend } from './types.ts'

export interface FileService {
  saveFile(fileName: string, fileData: Uint8Array | string): Promise<unknown>
  readFile(filePath: string, encoding?: 'base64'): Promise<unknown>
  copyFile(srcPath: string): Promise<unknown>
  getFiles(): Promise<unknown>
}

export function createFileService(backend: Backend): FileService {
  return {
    saveFile: (fileName, fileData) => backend.invoke('file:save', fileName, fileData),
    readFile: (filePath, encoding) =>
      encoding ? backend.invoke('file:read', filePath, encoding) : backend.invoke('file:read', filePath),
    copyFile: (srcPath) => backend.invoke('file:copy', srcPath),
    getFiles: () => backend.invoke('file:get-all')
  }
}
