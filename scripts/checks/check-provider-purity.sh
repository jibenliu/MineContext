#!/usr/bin/env bash
# Provider 纯净性检查。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python，
# 也不用为了「扫一段源码」启动一个 shell 进程。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-provider-purity
