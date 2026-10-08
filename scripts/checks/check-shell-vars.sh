#!/usr/bin/env bash
# shell 脚本里的变量展开：变量名后面**紧跟全角标点**会被 bash 3.2 当成变量名的一部分。
#
# macOS 自带 bash 3.2，`"$path（数据目录）"` 会展开成 `path（...` 这个变量名，
# 在脚本的 `set -u` 下直接 `unbound variable` 退出 —— 而报错信息里的变量名
# 带着乱码字节，非常难查（只能靠展开检查发现）。
#
# `"${path}（...）"` 是正确写法。这条守卫只拦这一种形态，不检查别的 shell 风格问题。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-shell-vars
