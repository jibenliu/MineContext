// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import type { Vault } from '@renderer/types/vault'

const logger = getLogger('vault')

import { VaultDocumentType } from '@shared/enums/global-enum'
import { getLogger } from '@shared/logger/renderer'

// Vault database operations
export const getAllVaults = async () => {
  try {
    const vaults = await window.dbAPI.getVaultsByDocumentType([VaultDocumentType.DailyReport, VaultDocumentType.Vaults])
    return vaults
  } catch (error) {
    logger.error('get all vaults::', error)
    throw error
  }
}

export const getVaultById = async (id: number) => {
  try {
    const vault = await window.dbAPI.getVaultById(id)
    return vault
  } catch (error) {
    logger.error('get vault by ID::', error)
    throw error
  }
}

export const getVaultByTitle = async (title: string) => {
  try {
    const vaults = await window.dbAPI.getVaultByTitle(title)
    return vaults
  } catch (error) {
    logger.error('get vault by title::', error)
    throw error
  }
}

export const updateVaultById = async (id: number, vault: Partial<Vault>) => {
  try {
    const result = await window.dbAPI.updateVaultById(id, vault)
    return result
  } catch (error) {
    logger.error('update vault by ID::', error)
    throw error
  }
}

export const insertVault = async (vault: Vault) => {
  try {
    const result = await window.dbAPI.insertVault(vault)
    return result
  } catch (error) {
    logger.error('insert vault::', error)
    throw error
  }
}

export const deleteVaultById = async (id: number) => {
  try {
    const result = await window.dbAPI.deleteVaultById(id)
    return result
  } catch (error) {
    logger.error('delete vault by ID::', error)
    throw error
  }
}

// New database operation methods
export const getVaultsByParentId = async (parentId: number | null) => {
  try {
    const vaults = await window.dbAPI.getVaultsByParentId(parentId)
    return vaults
  } catch (error) {
    logger.error('get vaults by parent ID::', error)
    throw error
  }
}

export const getFolders = async () => {
  try {
    const folders = await window.dbAPI.getFolders()
    return folders
  } catch (error) {
    logger.error('get folders::', error)
    throw error
  }
}

export const softDeleteVaultById = async (id: number) => {
  try {
    const result = await window.dbAPI.softDeleteVaultById(id)
    return result
  } catch (error) {
    logger.error('soft delete vault::', error)
    throw error
  }
}

export const restoreVaultById = async (id: number) => {
  try {
    const result = await window.dbAPI.restoreVaultById(id)
    return result
  } catch (error) {
    logger.error('restore vault::', error)
    throw error
  }
}

export const hardDeleteVaultById = async (id: number) => {
  try {
    const result = await window.dbAPI.hardDeleteVaultById(id)
    return result
  } catch (error) {
    logger.error('permanently delete vault::', error)
    throw error
  }
}

export const createFolder = async (title: string, parentId?: number) => {
  try {
    const result = await window.dbAPI.createFolder(title, parentId)
    return result
  } catch (error) {
    logger.error('create folder::', error)
    throw error
  }
}
