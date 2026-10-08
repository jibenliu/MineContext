#!/usr/bin/env bash
# macOS 产物校验：最低支持 13 —— 校验的是**真实产物**，不是配置字符串。
#
# 为什么不能只看 `.cargo/config.toml`：配置说 10.12、依赖的 object file 按 13.3 编时，
# 链接期只给警告，产物却声称支持 10.12 —— 这种漂移只有查产物本身才发现得了。
#
# 检查三件事：
#   1. 每个 Mach-O 产物的 LC_BUILD_VERSION.minos 都在 [13.0, 声明值] 区间内；
#   2. 声明值（.cargo/config.toml）与 `mc_common::platform::MIN_SUPPORTED_MACOS`
#      一致 —— 构建期与运行期必须是同一个数字；
#   3. 产物里不出现 ScreenCaptureKit（我们的采集走 CoreGraphics 的
#      CGWindowListCreateImage；引入 SCKit 会把可用范围推到 12.3+/14+）。
#
# 用法：scripts/check-macos-artifacts.sh [--min 13.0]
#
# 检查逻辑在 `apps/xtask`（Rust，产物检查并行跑），这里只是入口：
# 不需要系统 Python。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

MIN="13.0"
if [ "${1:-}" = "--min" ] && [ -n "${2:-}" ]; then
  MIN="$2"
fi

xtask_run check-macos-artifacts --min "$MIN"
