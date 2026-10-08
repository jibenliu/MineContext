#!/usr/bin/env bash
# 依赖方向检查：workspace 内部 crate 只能依赖「更底层」的 crate。
# 目的：mc-domain 保持纯逻辑（无 IO），mc-common 不依赖任何内部 crate。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-dep-direction
