#!/usr/bin/env bash
# 文档门禁：错误码与故障排查文档必须一致（36 个错误码逐一比对）。
set -uo pipefail

cd "$(dirname "$0")/.."

if ./scripts/checks/check-troubleshooting-freshness.sh; then
  echo "PASS: 故障排查文档与错误码一致"
else
  echo "FAIL: 故障排查文档与错误码不一致"
  exit 1
fi
