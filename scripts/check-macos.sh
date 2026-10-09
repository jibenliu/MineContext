#!/usr/bin/env bash
# macOS 相关门禁：最低支持范围（deployment target + CI 矩阵）与产物校验。
set -uo pipefail

cd "$(dirname "$0")/.."

failed=0
run() {
  local name="$1"
  shift
  if "$@" >/tmp/check-macos.$$.log 2>&1; then
    echo "PASS: ${name}"
  else
    echo "FAIL: ${name}"
    sed 's/^/      /' /tmp/check-macos.$$.log | head -20
    failed=1
  fi
}

run "最低支持范围（部署目标 13；CI runner ≥ 14）" ./scripts/checks/check-macos-target.sh
if [ "${MC_RUN_SMOKE:-0}" -eq 1 ]; then
  run "产物最低版本（minos 与链接期一致）" ./scripts/checks/check-macos-artifacts.sh
else
  echo "SKIP: 产物校验（提交/打版本前显式运行冒烟）"
fi

rm -f /tmp/check-macos.$$.log
if [ "${failed}" -ne 0 ]; then
  echo
  echo "macOS 门禁未通过"
  exit 1
fi
echo
echo "macOS 门禁全部通过"
