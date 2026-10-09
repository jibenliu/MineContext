#!/usr/bin/env bash
# package-release-binaries.sh：未知参数必须失败；--help 必须可用。
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
script="$root/scripts/package-release-binaries.sh"

if bash "$script" --help >/dev/null; then
  :
else
  echo 'FAIL: --help 应成功'
  exit 1
fi

if bash "$script" --unknown >/dev/null 2>&1; then
  echo 'FAIL: 未知参数应失败'
  exit 1
fi

echo 'PASS: package-release-binaries modes'
