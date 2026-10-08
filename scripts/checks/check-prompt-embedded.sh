#!/usr/bin/env bash
# 提示词必须编译进二进制，不能依赖运行时读取的外部配置文件。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python，
# 也不用为了「扫一段源码」启动一个 shell 进程。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-prompt-embedded
