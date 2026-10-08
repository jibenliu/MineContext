// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

// 渲染层日志：默认写 webview 控制台；探测到桌面外壳时**同时**转发给外壳落盘。
//
// 为什么两条路都要：控制台是开发时最顺手的出口，但打包版里根本看不到；日志文件
// 是出问题之后唯一能查的东西。因此外壳在就转发（外壳命令 `renderer_log` 写
// `<日志目录>/renderer.log`），外壳不在就只写控制台 —— 这里不 import 任何外壳
// API，日志器在任何环境都能用（jsdom 里也不需要替身）。

export type LogLevel = 'error' | 'warn' | 'info' | 'debug'

/** 落盘出口：由适配层在探测到外壳时装上；实现方自己负责不抛异常。 */
export type LogSink = (level: LogLevel, message: string) => void

/** 单条日志的上限：日志文件不该被一个巨大的对象刷爆。 */
const MAX_MESSAGE_CHARS = 2000

let sink: LogSink | undefined

/** 装/卸落盘出口（适配层在 bootstrap 时调用；测试用它注入假出口）。 */
export function setLogSink(next: LogSink | undefined): void {
  sink = next
}

function formatArg(arg: unknown): string {
  if (typeof arg === 'string') return arg
  if (arg instanceof Error) return `${arg.name}: ${arg.message}`
  if (arg === null || arg === undefined || typeof arg !== 'object') return String(arg)
  return safeStringify(arg)
}

/**
 * 对象转单行 JSON；循环引用标成 `[循环引用]` 而不是直接放弃整条日志。
 *
 *（`JSON.stringify` 遇到循环会抛，把对象直接丢给控制台/弹层的做法在日志里同样会炸：
 * 这里的选择是**尽量留下能看的内容**，实在不行才退化成 `String()`。）
 */
function safeStringify(arg: object): string {
  const seen = new WeakSet<object>()
  try {
    return (
      JSON.stringify(arg, (_key, value: unknown) => {
        if (typeof value === 'object' && value !== null) {
          if (seen.has(value)) return '[循环引用]'
          seen.add(value)
        }
        return value
      }) ?? String(arg)
    )
  } catch {
    // BigInt 等 stringify 不了的值：保住这一条日志，不要让调用点炸掉
    return String(arg)
  }
}

/** 拼出一条单行消息：控制台与文件里都能读，超长截断。 */
export function formatLogArgs(args: unknown[]): string {
  const text = args.map(formatArg).join(' ')
  return text.length > MAX_MESSAGE_CHARS ? `${text.slice(0, MAX_MESSAGE_CHARS)}…（已截断）` : text
}

/** 带 scope 的日志器：方法签名与业务代码的调用方式一致（`logger.error(...)`）。 */
export interface ScopedLogger {
  error(...args: unknown[]): void
  warn(...args: unknown[]): void
  info(...args: unknown[]): void
  debug(...args: unknown[]): void
}

/**
 * 取一个带 scope 前缀的日志器。
 * @param scope 作用域名，一般是组件或服务名。
 */
export function getLogger(scope?: string): ScopedLogger {
  const prefix = scope ? `[renderer - ${scope}]` : '[renderer]'

  // 出口在**调用时**取，而不是创建时：业务代码普遍在模块顶层 `getLogger(...)`，
  // 那时适配层还没 bootstrap，固定下来会让落盘永远装不上。
  const emit = (level: LogLevel) => {
    return (...args: unknown[]): void => {
      console[level](prefix, ...args)
      try {
        sink?.(level, `${prefix} ${formatLogArgs(args)}`)
      } catch {
        // 落盘是尽力而为：出口自己坏掉不该反过来影响业务，更不能在这里再写日志
        // （否则外壳不可用时会自激成日志风暴）。
      }
    }
  }

  return { error: emit('error'), warn: emit('warn'), info: emit('info'), debug: emit('debug') }
}

/** 渲染层默认日志器（无 scope）。 */
export const rendererLog = getLogger()
