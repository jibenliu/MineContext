// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import {
  Button,
  Card,
  Form,
  Input,
  Message,
  Modal,
  Popconfirm,
  Radio,
  Select,
  Space,
  Tooltip,
  Tree,
  Typography
} from '@arco-design/web-react'
import { IconDelete } from '@arco-design/web-react/icon'
import { deleteHomeTodo } from '@renderer/adapters/home-todos'
import addIcon from '@renderer/assets/icons/add.svg'
import doneIcon from '@renderer/assets/icons/done.svg'
import highPriorityIcon from '@renderer/assets/icons/high-priority.svg'
import lowPriorityIcon from '@renderer/assets/icons/low-priority.svg'
import mediumPriorityIcon from '@renderer/assets/icons/medium-priority.svg'
import copyIcon from '@renderer/assets/images/copy.svg'
import taskEmpty from '@renderer/assets/images/task-empty.svg'
import { TaskUrgency, TODO_LIST_STATUS } from '@renderer/constant/feed'
import { Task, useHomeInfo } from '@renderer/hooks/use-home-info'
import { useInitPrepareData } from '@renderer/hooks/use-init-prepare-data'
import { useI18n } from '@renderer/i18n'
import { useMemoizedFn } from 'ahooks'
import dayjs from 'dayjs'
import { FC, useEffect, useMemo, useRef, useState } from 'react'

const { Text } = Typography
const TextArea = Input.TextArea
const TreeNode = Tree.Node

function getTodoIcon(urgency: TaskUrgency) {
  switch (urgency) {
    case TaskUrgency.High:
      return highPriorityIcon
    case TaskUrgency.Medium:
      return mediumPriorityIcon
    case TaskUrgency.Low:
      return lowPriorityIcon
    case TaskUrgency.Done:
      return doneIcon
    default:
      return lowPriorityIcon
  }
}

// 模块级函数用不了 hook，因此这里只把「紧迫度 → 词条 key」的映射固定下来，
// 展示时再 t()（树节点的 key 也用同一份值：它在同一次渲染里是稳定的）。
function genTodoTitleKey(urgency: TaskUrgency) {
  switch (urgency) {
    case TaskUrgency.High:
      return 'home.todo.urgent'
    case TaskUrgency.Medium:
      return 'home.todo.medium'
    case TaskUrgency.Low:
      return 'home.todo.low'
    case TaskUrgency.Done:
      return 'home.todo.done'
    default:
      return 'home.todo.unknownPriority'
  }
}
export interface ToDoCardProps {
  selectedDays: string | null
}
const ToDoCard: FC<ToDoCardProps> = (props) => {
  const { selectedDays } = props
  const { t } = useI18n()
  const { tasks, toggleTaskStatus, updateTask, deleteTask, addTask, fetchTasks } = useHomeInfo()
  const hasTasks = useMemo(() => tasks.length > 0, [tasks])
  const [isTaskHover, setIsTaskHover] = useState<number | null>(null) // Edit task status
  const [isDeleting, setIsDeleting] = useState(false)
  const [copiedTaskId, setCopiedTaskId] = useState<number | null>(null) // Copied tooltip state
  const { deleteTodoList, data: todoListInitData } = useInitPrepareData()
  const [form] = Form.useForm()
  const filterDoneTasks = useMemo(
    () =>
      tasks.map((task) => {
        // Set urgency to done when task is done
        if (task.status === 1) {
          return {
            ...task,
            urgency: TaskUrgency.Done
          }
        }
        return task
      }),
    [tasks]
  )

  // Handle deleting a task
  const handleDeleteTask = useMemoizedFn(async (taskId: number) => {
    try {
      // 顺序与分支在 `adapters/home-todos.ts` 里（纯函数、有测试）：
      // 负数 id 是本地初始数据（教程占位），只删本地；真实条目先删服务端再删本地。
      await deleteHomeTodo(taskId, {
        deleteRemote: (id) => deleteTask(id),
        deleteLocal: async (id) => {
          await deleteTodoList(id)
        }
      })
      Message.success(t('home.todo.deleteSuccess'))
    } catch (error) {
      Message.error(t('home.todo.deleteFailed'))
    } finally {
      setIsDeleting(false)
      setIsTaskHover(null)
    }
  })

  const handleCopyContent = useMemoizedFn(async (content: string, taskId: number) => {
    try {
      await navigator.clipboard.writeText(content)

      // Show tooltip
      setCopiedTaskId(taskId)

      // Hide tooltip after 2 seconds
      setTimeout(() => {
        setCopiedTaskId(null)
      }, 2000)
    } catch (error) {
      Message.error(t('home.todo.copyFailed'))
    }
  })

  const renderTask = (task) => (
    <div
      key={task.id}
      className="flex w-full px-6 max-w-[1000px] items-center justify-between rounded-[4px]"
      onMouseEnter={() => {
        setIsTaskHover(task.id)
      }}
      onMouseLeave={() => {
        if (!isDeleting) {
          setIsTaskHover(null)
        }
      }}>
      <div className="gap-2 flex-1 flex items-center">
        <Radio className="self-start mt-0.5" checked={!!task.status} onClick={() => handleToggleTaskStatus(task)} />
        <div
          className={`font-roboto text-sm font-normal text-[var(--color-text-1)] leading-[22px] max-w-[800px] tracking-[0.042px] whitespace-normal break-words ${task.status && 'line-through'}`}
          onClick={() => handleEditToDoList(task)}>
          {task.content}
        </div>
      </div>
      <div className={`flex items-center ml-2 gap-3 ${isTaskHover === task.id ? 'opacity-100' : 'opacity-0'}`}>
        <Tooltip content={t('home.todo.copied')} position="top" popupVisible={copiedTaskId === task.id}>
          <Button
            type="text"
            size="small"
            className="[&_.arco-btn-size-small]: !w-[14px] !h-[14px]"
            icon={<img src={copyIcon} alt="copyIcon" className="w-[14px] h-[14px]" />}
            onClick={() => handleCopyContent(task.content, task.id)}
            disabled={!!task.status}
          />
        </Tooltip>
        <Popconfirm
          title={t('home.todo.confirmDeleteTitle')}
          content={t('home.todo.confirmDeleteContent')}
          onOk={() => handleDeleteTask(task.id)}
          onCancel={() => {
            setIsDeleting(false)
            setIsTaskHover(null)
          }}
          onVisibleChange={(visible) => {
            if (!visible) {
              setIsDeleting(false)
              setIsTaskHover(null)
            }
          }}
          okText={t('home.todo.confirm')}
          cancelText={t('home.todo.cancel')}>
          <Button
            type="text"
            size="small"
            icon={<IconDelete />}
            className="[&_.arco-btn-size-small]: !w-[13px] !h-[13px]"
            style={{ color: 'rgb(var(--danger-6))' }}
            onClick={() => {
              setIsDeleting(true)
            }}
          />
        </Popconfirm>
      </div>
    </div>
  )

  function buildTodoTree(tasks: Task[]) {
    const rootTitle = (urgency: TaskUrgency) => (
      <div className="flex items-center gap-2">
        <img src={getTodoIcon(urgency)} alt="todoIcon" />
        <div
          className="font-roboto text-sm text-[var(--color-text-1)] leading-[22px] tracking-[0.042px]"
          style={{
            fontWeight: 500
          }}>
          {t(genTodoTitleKey(urgency))}
        </div>
      </div>
    )

    const baseTree: { title: React.ReactNode; key: string; children: { title: React.ReactNode; key: string }[] }[] = [
      {
        title: rootTitle(TaskUrgency.High),
        key: genTodoTitleKey(TaskUrgency.High),
        children: []
      },
      {
        title: rootTitle(TaskUrgency.Medium),
        key: genTodoTitleKey(TaskUrgency.Medium),
        children: []
      },
      {
        title: rootTitle(TaskUrgency.Low),
        key: genTodoTitleKey(TaskUrgency.Low),
        children: []
      },
      {
        title: rootTitle(TaskUrgency.Done),
        key: genTodoTitleKey(TaskUrgency.Done),
        children: []
      }
    ]
    tasks.forEach((task) => {
      const taskTitle = renderTask(task)
      const nodeParam = {
        title: taskTitle,
        key: String(task.id)
      }
      switch (task.urgency) {
        case TaskUrgency.High:
          baseTree[0].children.push(nodeParam)
          break
        case TaskUrgency.Medium:
          baseTree[1].children.push(nodeParam)
          break
        case TaskUrgency.Low:
          baseTree[2].children.push(nodeParam)
          break
        case TaskUrgency.Done:
          baseTree[3].children.push(nodeParam)
          break
      }
    })
    const retTree = baseTree.filter((node) => node.children.length > 0)
    return (
      <Tree autoExpandParent blockNode actionOnClick="expand">
        {retTree.map((node) => (
          <TreeNode key={node.key} title={node.title}>
            {node.children.map((child) => (
              <TreeNode
                key={child.key}
                title={child.title}
                className="[&_.arco-tree-node-indent]:hidden [&_.arco-tree-node-switcher]:hidden"
              />
            ))}
          </TreeNode>
        ))}
      </Tree>
    )
  }

  const [status, setStatus] = useState(TODO_LIST_STATUS.Create)
  const [visible, setVisible] = useState(false)
  const handleCreateToDoList = useMemoizedFn(() => {
    setStatus(TODO_LIST_STATUS.Create)
    setVisible(true)
  })
  const timerRef = useRef<NodeJS.Timeout>(null)
  const handleToggleTaskStatus = useMemoizedFn(async (task) => {
    let id = task.id

    if (todoListInitData.some((v) => v.id === task.id)) {
      deleteTodoList(task.id)
      id = await addTask({
        content: task.content,
        urgency: task.urgency,
        status: 0
      })
    }
    timerRef.current = setTimeout(() => {
      toggleTaskStatus(id)
    }, 300)
  })
  const handleEditToDoList = useMemoizedFn((task) => {
    setStatus(TODO_LIST_STATUS.Edit)
    setVisible(true)
    timerRef.current = setTimeout(() => {
      form.setFieldsValue(task)
    }, 100)
  })
  const createTodoList = useMemoizedFn(async () => {
    try {
      await form.validate()
      const values = form.getFieldsValue()
      await addTask({
        content: values.content,
        urgency: values.urgency
      })
      Message.success(t('home.todo.addSuccess'))
    } catch (error: any) {
      Message.error(error.message || '')
    }
  })

  const editTodoList = useMemoizedFn(async () => {
    try {
      await form.validate()
      const values = form.getFieldsValue()
      if (todoListInitData.some((v) => v.id === values.id)) {
        deleteTodoList(values.id)
        await addTask({
          content: values.content,
          urgency: values.urgency
        })
      } else {
        await updateTask(values.id, {
          content: values.content,
          urgency: values.urgency
        })
      }
      Message.success(t('home.todo.updateSuccess'))
    } catch (error: any) {
      Message.error(error.message || '')
    }
  })

  const handleSave = useMemoizedFn(async () => {
    try {
      if (status === TODO_LIST_STATUS.Create) {
        await createTodoList()
      } else {
        await editTodoList()
      }
      setVisible(false)
    } catch (e: any) {
      Message.error(e.message || '')
    } finally {
      clearTimeout(timerRef.current!)
    }
  })

  useEffect(() => {
    // Need to refresh the task list after clicking a date on the heatmap
    if (selectedDays) {
      fetchTasks(dayjs(selectedDays))
    }
  }, [selectedDays, fetchTasks])

  return (
    <>
      <Card
        className="flex max-h-[460px] p-3 flex-col items-start gap-4 self-stretch rounded-[10px] border border-[var(--color-border-2)] bg-white w-full"
        headerStyle={{
          width: '100%'
        }}
        title={
          <div className="flex flex-1 justify-between items-center w-full">
            <Space style={{ marginTop: 5 }}>
              <div className="flex px-[2px] justify-center items-center gap-[4px] rounded-[2px] bg-gradient-to-l from-[rgba(239,251,248,0.5)] to-[#F5FBEF]">
                <div className="mr-[0.3em] font-['Roboto'] text-[15px] font-extralight leading-[22px] tracking-[0.045px] bg-gradient-to-l from-[#007740] to-[#D0B400] bg-clip-text text-transparent">
                  {t('home.todo.title')}
                </div>
              </div>
              <div className="text-[var(--color-text-1)] font-['Roboto'] text-sm font-medium leading-[22px] tracking-[0.042px]">
                {t('home.todo.today')}
              </div>
            </Space>
            <img src={addIcon} alt="" onClick={handleCreateToDoList} className="cursor-pointer" />
          </div>
        }
        bodyStyle={{
          alignItems: hasTasks ? 'flex-start' : 'center',
          justifyContent: hasTasks ? 'flex-start' : 'center',
          width: '100%',
          overflow: 'auto',
          scrollbarWidth: 'none',
          marginTop: '-10px'
        }}>
        <div className={`flex h-[340px] max-h-[340px] flex-col gap-4 self-stretch`}>
          {hasTasks ? (
            <div>{buildTodoTree([...(todoListInitData as any), ...filterDoneTasks])}</div>
          ) : (
            <div className="flex flex-col items-center justify-center pt-[60px] pb-[60px] text-center">
              <img src={taskEmpty} alt="empty" className="w-20 h-20 mb-4" />
              <Text type="secondary">{t('home.todo.empty')}</Text>
            </div>
          )}
        </div>
      </Card>
      {/* Edit task modal */}
      <Modal
        title={status === TODO_LIST_STATUS.Create ? t('home.todo.addTitle') : t('home.todo.editTitle')}
        visible={visible}
        onOk={handleSave}
        onCancel={() => setVisible(false)}
        okText={status === TODO_LIST_STATUS.Create ? t('home.todo.add') : t('home.todo.update')}
        cancelText={t('home.todo.cancel')}
        unmountOnExit>
        <Form
          layout="vertical"
          form={form}
          initialValues={{ urgency: TaskUrgency.Low }}
          className="[&_.arco-form-label-item>label]:!flex">
          <Form.Item field="id" noStyle>
            <Input className="hidden" />
          </Form.Item>
          <Form.Item
            label={t('home.todo.content')}
            field="content"
            rules={[{ required: true, message: t('home.todo.contentRequired') }]}>
            <TextArea autoSize placeholder={t('home.todo.contentPlaceholder')} />
          </Form.Item>
          <Form.Item label={t('home.todo.priority')} field="urgency">
            <Select>
              <Select.Option value={TaskUrgency.High}>{t('home.todo.urgent')}</Select.Option>
              <Select.Option value={TaskUrgency.Medium}>{t('home.todo.medium')}</Select.Option>
              <Select.Option value={TaskUrgency.Low}>{t('home.todo.low')}</Select.Option>
            </Select>
          </Form.Item>
        </Form>
      </Modal>
    </>
  )
}
export { ToDoCard }
