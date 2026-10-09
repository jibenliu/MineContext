#!/usr/bin/env bash
# 外壳命令必须出现在 capabilities 里：Tauri 2 默认拒绝未授权 invoke。
# 漏掉 get_runtime 时 daemon 已在听端口，渲染层却永远读不到 runtime →
# 「无法连接本地服务」。
set -euo pipefail
cd "$(dirname "$0")/../.."

caps='src-tauri/capabilities/default.json'
perms='src-tauri/permissions/shell-commands.toml'

required=(
  allow-get-runtime
  allow-renderer-log
  allow-tray-recording-status
  allow-launch-at-login
  allow-set-launch-at-login
  allow-clipboard-write-text
)

fail=0
for id in "${required[@]}"; do
  if ! grep -q "\"${id}\"" "$caps"; then
    echo "FAIL: ${caps} 缺少权限 ${id}"
    fail=1
  fi
done

if ! grep -q 'commands.allow = \["get_runtime"\]' "$perms"; then
  echo "FAIL: ${perms} 必须 allow get_runtime"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "PASS: Tauri 外壳 capabilities 含 get_runtime 等渲染层必需命令"
