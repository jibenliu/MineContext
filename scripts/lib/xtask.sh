#!/usr/bin/env bash
# scripts/lib/xtask.sh —— 守卫与测试脚本共用：构建一次 xtask，之后直接跑二进制。
#
# 为什么需要它：`cargo run` 每次都要做一遍指纹检查与锁等待（这台机器上约 7~9 秒），
# 9 条守卫串起来光这项开销就一分多钟。这里改成「源码比二进制新才构建」。
#
# 用法（在被 source 的脚本里）：. "$(dirname "$0")/../lib/xtask.sh"
#                                xtask_run check-comment-style
MC_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

xtask_run() {
  local bin="${MC_ROOT}/target/debug/xtask"
  local newest=""
  if [ -d "${MC_ROOT}/apps/xtask/src" ]; then
    newest="$(ls -t "${MC_ROOT}"/apps/xtask/src/*.rs "${MC_ROOT}"/apps/xtask/Cargo.toml 2>/dev/null | head -1)"
  fi
  if [ ! -x "${bin}" ] || { [ -n "${newest}" ] && [ "${newest}" -nt "${bin}" ]; }; then
    (cd "${MC_ROOT}" && cargo build -q -p xtask) || return 1
  fi
  "${bin}" "$@"
}
