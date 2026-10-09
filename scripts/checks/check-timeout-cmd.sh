#!/usr/bin/env bash
# 锁定 rust-tests-parallel.sh 的 timeout_cmd：在没有 GNU timeout 时必须丢掉秒数，
# 绝不能把「2700」当成可执行文件（macos-14 CI 曾因此整段测试挂掉）。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

# 源码必须包含「数字参数则 shift」的回退分支
grep -q 'shift' "$ROOT/scripts/tests/rust-tests-parallel.sh"
grep -Eq 'timeout_cmd\(\)' "$ROOT/scripts/tests/rust-tests-parallel.sh"

# 行为复现：没有 timeout/gtimeout 时，秒数不能被执行
timeout_cmd() {
  if [ "$#" -ge 1 ] && [[ "$1" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
    shift
  fi
  "$@"
}
out="$(timeout_cmd 2700 printf ok)"
test "$out" = "ok"
echo "check-timeout-cmd: ok"
