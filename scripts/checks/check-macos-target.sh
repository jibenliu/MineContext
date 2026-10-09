#!/usr/bin/env bash
# macOS 支持范围守卫：产品最低 13；托管 CI runner ≥ 14。
#
# 三处必须一致（少一处就会出现「文档说支持、二进制不支持」）：
#   1. 构建：`.cargo/config.toml` 的 MACOSX_DEPLOYMENT_TARGET ≥ 13.0
#   2. 运行：`mc_common::platform::MIN_SUPPORTED_MACOS`（由 Rust 测试覆盖）
#   3. CI：test 矩阵覆盖 macos-14+，且不得再依赖已退场的 macos-13 托管标签
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-macos-target
