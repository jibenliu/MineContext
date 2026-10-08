#!/usr/bin/env bash
# 前端样式里的颜色变量必须能被解析、并且随主题走。
#
# 拦的是两类「静默失效」（浏览器不报错，只是声明不生效）：
#   1. `var(--x)` 的 `--x` 在源码里没有定义（名字写错、或引用了别的设计系统的变量）；
#   2. Arco 的调色板变量（`--primary-6` 等）存的是 `R, G, B` 三元组而不是颜色，
#      必须写成 `rgb(var(--primary-6))` —— 裸用时实心按钮会变透明、白字看不见。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-theme-tokens
