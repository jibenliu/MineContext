// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

/**
 * TypeScript type definition corresponding to the 'activity' table.
 * Field types map one-to-one with SQL types:
 * - INTEGER → number (auto-incrementing ID is a number)
 * - TEXT → string (text content)
 * - JSON → generic object type (Record<string, any>, can be refined if the structure is known)
 * - DATETIME → string (datetime string, usually in ISO format like '2025-09-28 12:00:00')
 */
// 这是**环境声明**（本文件没有 import/export）：渲染层各文件直接用 `Activity`，
// 因此单文件视角下它「没人用」，此处显式豁免。
// eslint-disable-next-line @typescript-eslint/no-unused-vars
interface Activity {
  // Auto-incrementing primary key (integer)
  id: number
  // Activity title (text)
  title: string
  // Activity content (text)
  content: string
  // Resource information (JSON format, stores an object)
  resources: string // JSON 文本；形状由写入方决定，读取前一律走 parseJsonArray/withParsedResources
  // Start time (datetime string)
  start_time: string
  // End time (datetime string)
  end_time: string
  // Metadata (JSON format, stores extra information)
  metadata: string // JSON 文本，字段随活动类型变化
}
