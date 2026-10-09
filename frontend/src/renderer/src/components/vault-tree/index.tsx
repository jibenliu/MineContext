// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Dropdown, Input, Menu, Message, Modal, Space, Typography } from '@arco-design/web-react'
import { PushDataTypes } from '@renderer/constant/feed'
import { useEvents } from '@renderer/hooks/use-events'
import { useNavigation } from '@renderer/hooks/use-navigation'
import { useVaults } from '@renderer/hooks/use-vault'
import { useI18n } from '@renderer/i18n'
import { VaultTreeNode } from '@renderer/types'
import { VaultDocumentType, VaultTitle } from '@shared/enums/global-enum'
import { getLogger } from '@shared/logger/renderer'
import { useMemoizedFn, useMount, useRequest } from 'ahooks'
import clsx from 'clsx'
import dayjs from 'dayjs'
import { get } from 'lodash'
import PQueue from 'p-queue'
import { CSSProperties, useEffect, useMemo, useRef, useState } from 'react'
import { NodeRendererProps, Tree } from 'react-arborist'

import addIcon from '/src/assets/icons/add.svg'
import deleteIcon from '/src/assets/icons/delete.svg'
import fileIcon from '/src/assets/icons/file.svg'
import folderOpenIcon from '/src/assets/icons/folder-open.svg'
import folderStrokedIcon from '/src/assets/icons/folder-stroked.svg'
import renameIcon from '/src/assets/icons/rename.svg'

const queue = new PQueue({ concurrency: 2 })
const { Text, Ellipsis } = Typography
const logger = getLogger('VaultTree')

const Node = ({
  node,
  dragHandle,
  onImportLink
}: NodeRendererProps<VaultTreeNode> & { onImportLink?: (parentId: number) => void }) => {
  const { t } = useI18n()
  const isFolder = node.data.is_folder === 1
  const { deleteVault, createFolder, addVault, getVaultPath } = useVaults()
  const { navigateToVault, isVaultActive } = useNavigation()
  const [visible, setVisible] = useState(false)
  const [isFolderHover, setIsFolderHover] = useState(false)

  const dropList = (
    <Menu
      onClickMenuItem={async (key) => {
        setVisible(false)
        if (key === 'rename') {
          node.edit()
        } else if (key === 'delete') {
          deleteVault(node.data.id)
        } else if (key === 'new-folder') {
          const path = getVaultPath(node.data.id)
          if (path && path.length >= 6) {
            Message.warning(t('vault.tree.maxDepth'))
            return
          }
          await createFolder('Untitled', node.data.id)
          if (!node.isOpen) {
            node.toggle()
          }
        } else if (key === 'new-document') {
          await addVault({
            title: 'Untitled',
            content: '',
            parent_id: node.data.id
          })
          if (!node.isOpen) {
            node.toggle()
          }
        } else if (key === 'import-link') {
          onImportLink?.(node.data.id)
          if (!node.isOpen) {
            node.toggle()
          }
        }
      }}>
      <Menu.Item key="rename" className="flex items-center">
        <img src={renameIcon} className="w-[16px]" style={{ marginRight: '6px' }} />
        {t('vault.tree.rename')}
      </Menu.Item>
      <Menu.Item key="delete" className="flex items-center">
        <img src={deleteIcon} className="w-[16px]" style={{ marginRight: '6px' }} />
        {t('vault.tree.delete')}
      </Menu.Item>
      {isFolder && (
        <>
          <Menu.Item key="new-folder" className="flex items-center">
            <img src={folderStrokedIcon} className="w-[16px]" style={{ marginRight: '6px' }} />
            {t('vault.tree.newFolder')}
          </Menu.Item>
          <Menu.Item key="new-document" className="flex items-center">
            <img src={fileIcon} className="w-[16px]" style={{ marginRight: '6px' }} />
            {t('vault.tree.newDocument')}
          </Menu.Item>
          <Menu.Item key="import-link" className="flex items-center">
            <img src={fileIcon} className="w-[16px]" style={{ marginRight: '6px' }} />
            {t('vault.tree.importLink')}
          </Menu.Item>
        </>
      )}
    </Menu>
  )

  const title = useMemo(() => {
    const rawTitle = node.data.title ?? ''
    if (rawTitle.startsWith('Daily Report')) {
      const end = rawTitle.match(/\d{4}-\d{2}-\d{2}/)?.[0]
      return end ? dayjs(end).format('MMM D, YYYY') : rawTitle
    } else {
      return rawTitle
    }
  }, [node.data])

  return (
    <Dropdown trigger="contextMenu" droplist={dropList} onVisibleChange={setVisible} popupVisible={visible}>
      <div
        style={{
          padding: '8px 12px',
          paddingLeft: `${node.level * 24 + 12}px`,
          borderRadius: '6px',
          display: 'flex',
          alignItems: 'center',
          gap: '8px',
          cursor: 'pointer',
          backgroundColor: !isFolder && isVaultActive(node.data.id) ? 'var(--color-bg-4)' : 'transparent',
          fontWeight: !isFolder && isVaultActive(node.data.id) ? 500 : 400,
          transition: 'all 0.2s ease'
        }}
        ref={dragHandle}
        onClick={() => {
          if (isFolder) {
            node.toggle()
          } else {
            node.activate()
            navigateToVault(node.data.id)
          }
        }}
        onMouseEnter={(e) => {
          if (!(!isFolder && isVaultActive(node.data.id))) {
            e.currentTarget.style.backgroundColor = 'var(--color-bg-4)'
          }
        }}
        onMouseLeave={(e) => {
          setIsFolderHover(false)
          if (!(!isFolder && isVaultActive(node.data.id))) {
            e.currentTarget.style.backgroundColor = 'transparent'
          }
        }}>
        {isFolder ? (
          <>
            <img
              onMouseEnter={() => setIsFolderHover(true)}
              onMouseLeave={() => setIsFolderHover(false)}
              src={node.isOpen || isFolderHover ? folderOpenIcon : folderStrokedIcon}
              alt="folder"
              className="w-[16px] rounded-[4px] shrink-0"
            />
          </>
        ) : (
          <img src={fileIcon} alt="file" className="w-[16px] rounded-[4px] shrink-0" />
        )}
        {node.isEditing ? (
          <Input
            autoFocus
            defaultValue={node.data.title}
            onBlur={() => node.reset()}
            className="h-[25px]"
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                node.submit(e.currentTarget.value)
              } else if (e.key === 'Escape') {
                node.reset()
              }
            }}
          />
        ) : (
          <Ellipsis
            className="text-[13px] overflow-auto"
            showTooltip
            rows={1}
            style={{
              fontWeight: !isFolder && isVaultActive(node.data.id) ? 500 : 400
            }}>
            {title}
          </Ellipsis>
        )}
      </div>
    </Dropdown>
  )
}

const Sidebar = ({ className }: { className?: string }) => {
  const { t } = useI18n()
  const {
    vaults: treeData,
    updateVaultPosition,
    createFolder,
    addVault,
    renameVault: onRenameVault,
    updateVault,
    initVaults
  } = useVaults()

  const { feedEvents } = useEvents()
  // eventLoop every 10s
  const dailySummaryPushEvent = useMemo(() => {
    return (feedEvents || []).filter((event) => event.type === PushDataTypes.DAILY_SUMMARY_GENERATED)
  }, [feedEvents])
  const { run: getDailyReportDocument, data: dailyReportDocument } = useRequest(window.dbAPI.getVaultsByDocumentType, {
    manual: true
  })
  const { run: getSummaryFolder, data: summaryFolderId } = useRequest(
    async (title: VaultTitle) => {
      const folder = await window.dbAPI.getVaultByTitle(title)
      if (Array.isArray(folder) && folder.length > 0) {
        return get(folder, '0.id')
      }
      const res = await createFolder(title)
      return get(res, 'id')
    },
    { manual: true }
  )
  const updateVaultsParentId = useMemoizedFn(async (ids: number[]) => {
    if (!ids || ids.length === 0) {
      return
    }
    const tasks = ids.map((id) => {
      return async () => {
        await updateVault(id, { parent_id: summaryFolderId })
      }
    })

    try {
      await initVaults()
      await queue.addAll(tasks)
      logger.info('bind summary folder success')
    } catch (error) {
      logger.error('bind summary folder failed', error)
    } finally {
      await initVaults()
    }
  })

  useMount(() => {
    getDailyReportDocument(VaultDocumentType.DailyReport)
    getSummaryFolder(VaultTitle.Summary)
  })
  useEffect(() => {
    if (summaryFolderId) {
      const ids =
        dailyReportDocument?.filter((item) => item.parent_id === -1 || !item.parent_id)?.map((item) => item.id) || []
      updateVaultsParentId(ids)
    }
  }, [summaryFolderId, dailyReportDocument, updateVaultsParentId])

  useEffect(() => {
    if (dailySummaryPushEvent.length > 0 && summaryFolderId) {
      const summary = get(treeData, 'children')?.find((item) => item.title === VaultTitle.Summary)
      const exitIds = summary?.children?.map((item) => item.id) || []
      const ids = dailySummaryPushEvent.map((item) => Number(get(item, 'data.doc_id')))
      updateVaultsParentId(ids.filter((id) => !exitIds.includes(id)))
    }
  }, [dailySummaryPushEvent, summaryFolderId, treeData, updateVaultsParentId])

  const { navigateToVault } = useNavigation()
  const treeContainerRef = useRef<HTMLDivElement>(null)
  const [treeDimensions, setTreeDimensions] = useState({ width: 200, height: 600 })
  const [linkModalVisible, setLinkModalVisible] = useState(false)
  const [linkUrl, setLinkUrl] = useState('')
  const [linkImporting, setLinkImporting] = useState(false)
  const [linkParentId, setLinkParentId] = useState<number | null>(null)

  const openLinkModal = useMemoizedFn((parentId?: number | null) => {
    setLinkParentId(parentId ?? null)
    setLinkUrl('')
    setLinkModalVisible(true)
  })
  useEffect(() => {
    if (!treeContainerRef.current) return

    const updateSize = () => {
      const el = treeContainerRef.current
      if (!el) return
      setTreeDimensions({
        height: el.clientHeight,
        width: el.clientWidth
      })
    }

    // 初始化时先更新一次
    updateSize()

    const observer = new ResizeObserver(() => {
      updateSize()
    })

    observer.observe(treeContainerRef.current)

    // 清理 observer
    return () => observer.disconnect()
  }, [treeContainerRef])

  const onRename = ({ id, name }: { id: string; name: string }) => {
    onRenameVault(Number(id), name)
  }
  const onMove = async ({
    dragIds,
    parentId,
    index
  }: {
    dragIds: string[]
    parentId: string | null
    index: number
  }) => {
    const newParentId = parentId === null || parentId === undefined ? -1 : Number(parentId)
    for (const id of dragIds) {
      await updateVaultPosition(Number(id), { parent_id: newParentId, sort_order: index })
    }
  }

  const handleMenuClick = async (key: string) => {
    if (key === 'new-folder') {
      await createFolder('Untitled')
    } else if (key === 'new-document') {
      await addVault({ title: 'Untitled', content: '' })
    } else if (key === 'import-link') {
      openLinkModal(null)
    }
  }

  const handleImportLink = async () => {
    const url = linkUrl.trim()
    if (!url) {
      Message.warning(t('vault.tree.linkRequired'))
      return
    }
    if (!window.linkApi?.importUrl) {
      Message.error(t('vault.tree.linkFailed'))
      return
    }
    setLinkImporting(true)
    try {
      const result = await window.linkApi.importUrl(url, linkParentId)
      setLinkModalVisible(false)
      setLinkUrl('')
      setLinkParentId(null)
      await initVaults()
      navigateToVault(result.id)
      Message.success(t('vault.tree.linkImported'))
    } catch (error) {
      logger.error('import link failed', error)
      const detail =
        error && typeof error === 'object' && 'message' in error
          ? String((error as { message?: unknown }).message || '')
          : ''
      Message.error(detail || t('vault.tree.linkFailed'))
    } finally {
      setLinkImporting(false)
    }
  }

  const menu = (
    <Menu onClickMenuItem={handleMenuClick} className="w-[180px] text-[12px]">
      <Menu.Item key="new-folder" className="flex">
        <img src={folderStrokedIcon} style={{ width: '16px', marginRight: '6px' }} />
        {t('vault.tree.newFolder')}
      </Menu.Item>
      <Menu.Item key="new-document" className="flex">
        <img src={fileIcon} style={{ width: '16px', marginRight: '6px' }} />
        {t('vault.tree.newDocument')}
      </Menu.Item>
      <Menu.Item key="import-link" className="flex">
        <img src={fileIcon} style={{ width: '16px', marginRight: '6px' }} />
        {t('vault.tree.importLink')}
      </Menu.Item>
    </Menu>
  )

  return (
    <div
      className={clsx('flex flex-col min-h-[0]', className)}
      style={{ marginTop: 12, appRegion: 'no-drag' } as CSSProperties}>
      <div className="flex flex-col flex-1 min-h-[0]">
        {/* Vault notes section */}
        <div className="px-[12px] shrink-0">
          <Space direction="vertical" size={8} className="w-full">
            <div className="flex items-center justify-between">
              <Text className="text-[12px]" style={{ color: 'var(--color-text-2)' }}>
                {t('vault.tree.creation')}
              </Text>
              <div className="flex justify-end gap-[8px] flex-1 items-center">
                <Dropdown droplist={menu} trigger="click">
                  <img src={addIcon} alt="add" className="w-[12px] cursor-pointer" />
                </Dropdown>
              </div>
            </div>
          </Space>
        </div>

        <Modal
          title={t('vault.tree.importLink')}
          visible={linkModalVisible}
          onOk={() => void handleImportLink()}
          onCancel={() => {
            if (!linkImporting) {
              setLinkModalVisible(false)
              setLinkUrl('')
              setLinkParentId(null)
            }
          }}
          confirmLoading={linkImporting}
          okText={t('vault.tree.importLinkConfirm')}
          cancelText={t('common.cancel')}
          unmountOnExit>
          <Input
            value={linkUrl}
            onChange={setLinkUrl}
            placeholder={t('vault.tree.linkPlaceholder')}
            allowClear
            onPressEnter={() => void handleImportLink()}
          />
        </Modal>

        {/* Modern tree structure */}
        {/* 文字色必须走语义变量：节点标题在暗色主题下同样要可读，写死颜色会变成黑底黑字 */}
        <div
          className="text-[var(--color-text-1)] flex-1 min-h-[0]  [&_.arco-typography]: !font-[13px] overflow-auto"
          ref={treeContainerRef}>
          <Tree
            idAccessor={(data) => String(data?.id ?? '')}
            data={(treeData?.children || []).filter((node) => node && node.id !== undefined && node.id !== null)}
            onRename={onRename}
            onMove={onMove}
            width={treeDimensions.width}
            height={treeDimensions.height}
            rowHeight={32}
            indent={0}
            onActivate={(node) => {
              if (node.data.is_folder !== 1) {
                navigateToVault(node.data.id)
              }
            }}>
            {(props) => <Node {...props} onImportLink={openLinkModal} />}
          </Tree>
        </div>
      </div>
    </div>
  )
}

export default Sidebar
