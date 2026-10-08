// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { TODOActivity } from '@interface/db/todo'
import { TaskUrgency } from '@renderer/constant/feed'
import { useI18n } from '@renderer/i18n'
import { getLogger } from '@shared/logger/renderer'
import { useMount, useRequest } from 'ahooks'
import dayjs from 'dayjs'
import { useEffect, useState } from 'react'

const logger = getLogger('use-init-prepare-data')
const buildTodoList = (t: (key: string) => string) => [
  {
    id: -3, // 固定占位 ID（负数，不与真实自增 id 冲突）
    content: t('home.guide.tutorial'),
    created_at: dayjs().format('YYYY-MM-DD HH:mm:ss'),
    urgency: TaskUrgency.Low,
    start_time: dayjs().format('YYYY-MM-DD HH:mm:ss'),
    end_time: dayjs().format('YYYY-MM-DD 23:59:59'),
    assignee: 'system'
  },
  {
    id: -2,
    content: t('home.guide.screenMonitor'),
    created_at: dayjs().format('YYYY-MM-DD HH:mm:ss'),
    urgency: TaskUrgency.Low,
    start_time: dayjs().format('YYYY-MM-DD HH:mm:ss'),
    end_time: dayjs().format('YYYY-MM-DD 23:59:59'),
    assignee: 'system'
  },
  {
    id: -4,
    content: t('home.guide.chat'),
    created_at: dayjs().format('YYYY-MM-DD HH:mm:ss'),
    urgency: TaskUrgency.Low,
    start_time: dayjs().format('YYYY-MM-DD HH:mm:ss'),
    end_time: dayjs().format('YYYY-MM-DD 23:59:59'),
    assignee: 'system'
  }
]
const useInitPrepareData = () => {
  const { t } = useI18n()
  const TODOList = buildTodoList(t)
  const [todoList, setTodoList] = useState<TODOActivity[]>([])
  const { run, loading, data } = useRequest<TODOActivity[], any>(
    async () => {
      // 适配层还没装好时不要抛：启动期少一次初始化，比一个未处理的
      // rejection 更容易排查（后者只会让人以为功能坏了）。
      if (!window.screenMonitorAPI) {
        logger.warn('[mc] screenMonitorAPI 不可用，跳过首页初始化数据')
        return []
      }

      const isFinished = await window.screenMonitorAPI.getSettings<boolean>('todoList-finished')
      if (isFinished) {
        return []
      }

      const res = await window.screenMonitorAPI.getSettings<TODOActivity[]>('todoList')
      if (!res || !Array.isArray(res)) {
        await window.screenMonitorAPI.setSettings('todoList', TODOList)
        return TODOList as TODOActivity[]
      }
      return res
    },
    { manual: true }
  )
  const { run: deleteTodoList, data: resetData } = useRequest(
    async (id: number) => {
      const res = await window.screenMonitorAPI.getSettings<TODOActivity[]>('todoList')
      if (res && Array.isArray(res)) {
        const filteredList = res.filter((item) => item.id !== id)
        await window.screenMonitorAPI.setSettings('todoList', filteredList)
        if (filteredList.length <= 0) {
          await window.screenMonitorAPI.setSettings('todoList-finished', true)
        }
        return filteredList
      }
      return []
    },
    { manual: true }
  )
  const { run: editTodoList } = useRequest(
    async (activity: TODOActivity) => {
      const res = await window.screenMonitorAPI.getSettings<TODOActivity[]>('todoList')
      if (res && Array.isArray(res)) {
        const updatedList = res.map((item) => (item.id === activity.id ? activity : item))
        await window.screenMonitorAPI.setSettings('todoList', updatedList)
        return updatedList
      }
      return []
    },
    { manual: true }
  )
  useMount(() => {
    run()
  })

  useEffect(() => {
    if (data) {
      setTodoList(data)
    }
  }, [data])
  useEffect(() => {
    if (resetData) {
      setTodoList(resetData)
    }
  }, [resetData])

  return { loading, data: todoList, deleteTodoList, editTodoList }
}
export { useInitPrepareData }
