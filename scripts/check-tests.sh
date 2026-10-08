#!/usr/bin/env bash
# 测试门禁：规划里承诺的安全与隐私属性必须各有一条测试（11 条）。
set -uo pipefail

cd "$(dirname "$0")/.."

if ./scripts/checks/check-security-tests.sh; then
  echo "PASS: 安全属性各有测试（11 条）"
else
  echo "FAIL: 有安全属性缺少测试"
  exit 1
fi
