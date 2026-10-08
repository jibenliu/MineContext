// 首页待办的删除语义。
//
// 待办有**两个来源**：daemon 的 `/api/db/todos`（真实条目，id ≥ 0）与 settings 里的
// 本地初始数据（教程占位，id 为负数）。负数 id 在库里不存在，对它调删除接口必然失败 ——
// 负 id 是本地初始数据：先调服务端删必然失败，教程条目就永远删不掉、用户只看到「删除失败」。
//
// 这里把「先删谁、失败怎么办」抽成纯函数：顺序与分支都能在不渲染界面的前提下测到。

export interface HomeTodoDeleteDeps {
  /** 删除服务端条目（id ≥ 0 时才调用）。 */
  deleteRemote(id: number): Promise<unknown>
  /** 删除本地初始数据条目（settings）。 */
  deleteLocal(id: number): Promise<unknown>
}

/**
 * 删除一条首页待办。
 *
 * - **负数 id 只删本地**：那是教程占位数据，服务端没有对应行。
 * - 其余先删服务端，成功后再删本地 —— 反过来会出现「界面没了、库里还在」。
 * - 服务端失败时**不删本地**并把错误抛出去，让调用方如实提示。
 */
export async function deleteHomeTodo(taskId: number, deps: HomeTodoDeleteDeps): Promise<void> {
  if (taskId >= 0) {
    await deps.deleteRemote(taskId)
  }
  await deps.deleteLocal(taskId)
}
