// `window.dbAPI` 的形状。
//
// 方法名、参数顺序、返回形状必须与 preload 完全一致 ——
// use-home-info.ts / use-vault.ts / vault-thunk.ts 直接依赖它们。

import type { Backend } from './types.ts'

export interface DbApi {
  getAllActivities(): Promise<unknown>
  getNewActivities(startTime: string, endTime?: string): Promise<unknown>
  getLatestActivity(): Promise<unknown>

  getAllVaults(): Promise<unknown>
  getVaultsByParentId(parentId: number | null): Promise<unknown>
  getVaultById(id: number): Promise<unknown>
  getVaultByTitle(title: string): Promise<unknown>
  getVaultsByDocumentType(documentType: string | string[]): Promise<unknown>
  getFolders(): Promise<unknown>
  insertVault(vault: unknown): Promise<unknown>
  updateVaultById(id: number, vault: unknown): Promise<unknown>
  deleteVaultById(id: number): Promise<unknown>
  softDeleteVaultById(id: number): Promise<unknown>
  restoreVaultById(id: number): Promise<unknown>
  hardDeleteVaultById(id: number): Promise<unknown>
  createFolder(title: string, parentId?: number): Promise<unknown>

  getTasks(startTime: string, endTime: string): Promise<unknown>
  addTask(task: unknown): Promise<unknown>
  updateTask(id: number, task: unknown): Promise<unknown>
  deleteTask(id: number): Promise<unknown>
  toggleTaskStatus(id: number): Promise<unknown>

  getAllTips(): Promise<unknown>
  getHeatmapData(startTime: number, endTime: number): Promise<unknown>
}

export function createDbApi(backend: Backend): DbApi {
  return {
    getAllActivities: () => backend.invoke('database:get-all-activities'),
    getNewActivities: (startTime, endTime) => backend.invoke('database:get-new-activities', startTime, endTime),
    getLatestActivity: () => backend.invoke('database:get-latest-activity'),

    getAllVaults: () => backend.invoke('database:get-all-vaults'),
    getVaultsByParentId: (parentId) => backend.invoke('database:get-vaults-by-parent-id', parentId),
    getVaultById: (id) => backend.invoke('database:get-vault-by-id', id),
    getVaultByTitle: (title) => backend.invoke('database:get-vault-by-title', title),
    getVaultsByDocumentType: (documentType) => backend.invoke('database:get-vaults-by-document-type', documentType),
    getFolders: () => backend.invoke('database:get-folders'),
    insertVault: (vault) => backend.invoke('database:insert-vault', vault),
    updateVaultById: (id, vault) => backend.invoke('database:update-vault-by-id', id, vault),
    deleteVaultById: (id) => backend.invoke('database:delete-vault-by-id', id),
    softDeleteVaultById: (id) => backend.invoke('database:soft-delete-vault-by-id', id),
    restoreVaultById: (id) => backend.invoke('database:restore-vault-by-id', id),
    hardDeleteVaultById: (id) => backend.invoke('database:hard-delete-vault-by-id', id),
    createFolder: (title, parentId) => backend.invoke('database:create-folder', title, parentId),

    getTasks: (startTime, endTime) => backend.invoke('database:get-all-tasks', startTime, endTime),
    addTask: (task) => backend.invoke('database:add-task', task),
    updateTask: (id, task) => backend.invoke('database:update-task', id, task),
    deleteTask: (id) => backend.invoke('database:delete-task', id),
    toggleTaskStatus: (id) => backend.invoke('database:toggle-task-status', id),

    getAllTips: () => backend.invoke('database:get-all-tips'),
    getHeatmapData: (startTime, endTime) => backend.invoke('heatmap:get-data', startTime, endTime)
  }
}
