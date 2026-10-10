// 首次引导清单：步骤门闩与「是否还要显示」的纯判断。
//
// 为什么单独抽：首启要把「屏幕录制权限 → API Key → 开始录制 → 首张截图」钉成
// 可测契约；UI 只负责渲染与 CTA，不能把门闩写进组件回调。完成态进 redux-persist，
// 避免每次冷启动再弹一遍。

export type FirstRunStepId = 'permission' | 'apiKey' | 'startRecording' | 'firstScreenshot'

export type FirstRunStepStatus = 'done' | 'current' | 'locked'

export interface FirstRunSnapshot {
  permissionGranted: boolean
  apiKeyConfigured: boolean
  isRecording: boolean
  hasScreenshot: boolean
  /** 用户主动清掉「等首张截图」的卡住态（TCC / 未开始等，见 capture UX）。 */
  waitingCleared: boolean
  /** 已完成或用户跳过，持久化后不再显示。 */
  completedPersisted: boolean
}

export interface FirstRunStepView {
  id: FirstRunStepId
  status: FirstRunStepStatus
}

const STEP_ORDER: FirstRunStepId[] = ['permission', 'apiKey', 'startRecording', 'firstScreenshot']

function stepDone(id: FirstRunStepId, snap: FirstRunSnapshot): boolean {
  switch (id) {
    case 'permission':
      return snap.permissionGranted
    case 'apiKey':
      return snap.apiKeyConfigured
    case 'startRecording':
      // 已有截图说明录制曾经跑通过，不必再强迫点一次开始。
      return snap.isRecording || snap.hasScreenshot
    case 'firstScreenshot':
      return snap.hasScreenshot || snap.waitingCleared
  }
}

/** 按门闩排出四步的状态；恰好一步为 current（全完成时全是 done）。 */
export function deriveFirstRunSteps(snap: FirstRunSnapshot): FirstRunStepView[] {
  let currentAssigned = false
  return STEP_ORDER.map((id, index) => {
    if (stepDone(id, snap)) {
      return { id, status: 'done' as const }
    }
    const priorsDone = STEP_ORDER.slice(0, index).every((prior) => stepDone(prior, snap))
    if (!priorsDone) {
      return { id, status: 'locked' as const }
    }
    if (!currentAssigned) {
      currentAssigned = true
      return { id, status: 'current' as const }
    }
    return { id, status: 'locked' as const }
  })
}

export function isFirstRunAllDone(snap: FirstRunSnapshot): boolean {
  return STEP_ORDER.every((id) => stepDone(id, snap))
}

/** 未持久化且尚未全部完成时显示轻量清单。 */
export function shouldShowFirstRunChecklist(snap: FirstRunSnapshot): boolean {
  if (snap.completedPersisted) return false
  return !isFirstRunAllDone(snap)
}

/** 当前应行动的一步；全完成时返回 null。 */
export function currentFirstRunStep(snap: FirstRunSnapshot): FirstRunStepId | null {
  return deriveFirstRunSteps(snap).find((s) => s.status === 'current')?.id ?? null
}
