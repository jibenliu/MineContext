// 首次引导清单：步骤门闩与完成持久化。用 node --test 跑。

import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  deriveFirstRunSteps,
  type FirstRunSnapshot,
  isFirstRunAllDone,
  shouldShowFirstRunChecklist
} from '../first-run-checklist.ts'

const base = (over: Partial<FirstRunSnapshot> = {}): FirstRunSnapshot => ({
  permissionGranted: false,
  apiKeyConfigured: false,
  isRecording: false,
  hasScreenshot: false,
  waitingCleared: false,
  completedPersisted: false,
  ...over
})

test('未授权时当前步是屏幕录制权限，后续锁定', () => {
  const steps = deriveFirstRunSteps(base())
  assert.deepEqual(
    steps.map((s) => [s.id, s.status]),
    [
      ['permission', 'current'],
      ['apiKey', 'locked'],
      ['startRecording', 'locked'],
      ['firstScreenshot', 'locked']
    ]
  )
})

test('有权限未配密钥 → 当前步是 API Key', () => {
  const steps = deriveFirstRunSteps(base({ permissionGranted: true }))
  assert.equal(steps.find((s) => s.id === 'permission')?.status, 'done')
  assert.equal(steps.find((s) => s.id === 'apiKey')?.status, 'current')
  assert.equal(steps.find((s) => s.id === 'startRecording')?.status, 'locked')
})

test('密钥已配、未录制 → 当前步是开始录制', () => {
  const steps = deriveFirstRunSteps(base({ permissionGranted: true, apiKeyConfigured: true }))
  assert.equal(steps.find((s) => s.id === 'startRecording')?.status, 'current')
  assert.equal(steps.find((s) => s.id === 'firstScreenshot')?.status, 'locked')
})

test('已在录制、尚无截图 → 当前步是确认首张截图', () => {
  const steps = deriveFirstRunSteps(base({ permissionGranted: true, apiKeyConfigured: true, isRecording: true }))
  assert.equal(steps.find((s) => s.id === 'startRecording')?.status, 'done')
  assert.equal(steps.find((s) => s.id === 'firstScreenshot')?.status, 'current')
})

test('已有截图 → 全部完成（开始录制也算完成）', () => {
  const snap = base({
    permissionGranted: true,
    apiKeyConfigured: true,
    hasScreenshot: true
  })
  assert.equal(isFirstRunAllDone(snap), true)
  assert.ok(deriveFirstRunSteps(snap).every((s) => s.status === 'done'))
})

test('清除等待态可完成首张截图步（不必真有截图）', () => {
  const snap = base({
    permissionGranted: true,
    apiKeyConfigured: true,
    isRecording: true,
    waitingCleared: true
  })
  assert.equal(isFirstRunAllDone(snap), true)
  assert.equal(stepsStatus(snap, 'firstScreenshot'), 'done')
})

test('未持久化完成时显示清单；全部完成或已持久化则隐藏', () => {
  assert.equal(shouldShowFirstRunChecklist(base()), true)
  assert.equal(
    shouldShowFirstRunChecklist(
      base({
        permissionGranted: true,
        apiKeyConfigured: true,
        hasScreenshot: true
      })
    ),
    false
  )
  assert.equal(shouldShowFirstRunChecklist(base({ completedPersisted: true })), false)
})

test('持久化完成后即使状态回退也不再显示', () => {
  assert.equal(
    shouldShowFirstRunChecklist(
      base({
        completedPersisted: true,
        permissionGranted: false,
        apiKeyConfigured: false
      })
    ),
    false
  )
})

function stepsStatus(snap: FirstRunSnapshot, id: string): string | undefined {
  return deriveFirstRunSteps(snap).find((s) => s.id === id)?.status
}
