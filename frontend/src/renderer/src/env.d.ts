// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

/// <reference types="vite/client" />
/// <reference types="./types/shell-globals.d.ts" />

// 本文件没有 import/export，下面的声明直接进全局作用域
// （不要再包一层 `declare global`：在非模块文件里它不生效）。
interface Window {
  /**
   * 桌面外壳提供的 daemon 运行时信息（端口 + token）。
   * 读的是 daemon 写的 `runtime.json`；没有它就无法切到 rust 后端。
   */
  mcRuntime?: {
    get?: () => Promise<{ port: number; token: string } | null>
  }
}
