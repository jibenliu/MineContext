#!/usr/bin/env bash
# 验**打包产物**里的 daemon：不解压、不重编，直接跑 `.app` 里那份二进制。
#
# 为什么单独一条：`smoke-daemon.sh` 验的是 `target/` 里的构建产物，
# 而交付给用户的是 `.app/Contents/Resources/backend/mc-daemon`。
# 这两者会不一致：复制脚本若「文件在就跳过」，产物里会留下过期二进制
# （少接口、不处理 SIGTERM），打包却显示成功 —— 所以直接跑 `.app` 里那份。
#
# 前置：先产出 `.app`（`./scripts/package-macos-tauri.sh`）。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

app="${1:-frontend/dist/mac/MineContext.app}"
if [ ! -d "$app" ]; then
  echo "SKIP: 还没有打包产物（${app}）。先跑：./scripts/package-macos-tauri.sh"
  exit 0
fi

daemon="$app/Contents/Resources/backend/mc-daemon"
if [ ! -x "$daemon" ]; then
  echo "FAIL: 产物里没有可执行的 daemon：$daemon"
  exit 1
fi

work_dir="$(mktemp -d)"
body_file="$(mktemp)"
log_file="$(mktemp)"
daemon_pid=""

cleanup() {
  if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill -TERM "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  # 同 launch-check：先把只读子目录的写权限恢复，再删。
  chmod -R u+w "$work_dir" 2>/dev/null || true
  rm -rf "$work_dir" 2>/dev/null || echo "提示：临时目录未能删净：$work_dir"
  rm -f "$body_file" "$log_file" 2>/dev/null || true
}
trap cleanup EXIT

echo "== 跑产物里的 daemon（${daemon}） =="
"$daemon" --data-dir "$work_dir" --port 0 >"$log_file" 2>&1 &
daemon_pid=$!

for _ in $(seq 1 150); do
  [ -f "$work_dir/runtime.json" ] && break
  kill -0 "$daemon_pid" 2>/dev/null || { echo "FAIL: daemon 提前退出"; tail -20 "$log_file"; exit 1; }
  sleep 0.2
done
[ -f "$work_dir/runtime.json" ] || { echo "FAIL: 30 秒内没有 runtime.json"; tail -20 "$log_file"; exit 1; }

port="$(xtask_run json-field "$work_dir/runtime.json" port)"
token="$(xtask_run json-field "$work_dir/runtime.json" token)"
base="http://127.0.0.1:${port}"
echo "   port=$port"

fail=0
# 前端的后端切换依赖这几条：少一条就是「界面打不开/设置页空白」
for path in /api/health /api/diagnostics /api/v1/diagnostics/export \
            /api/model_settings/get /api/capture/config /api/capture/status \
            /api/db/vaults /api/monitoring/recording-stats; do
  status="$(curl -s -o "$body_file" -w '%{http_code}' -H "x-mc-token: $token" "${base}${path}")"
  if [ "$status" != "200" ]; then
    echo "FAIL: $path → HTTP $status"; fail=1; continue
  fi
  # 健康检查是裸对象（公开面），其余都走统一信封
  if [ "$path" != "/api/health" ] && ! grep -q '"code":0' "$body_file"; then
    echo "FAIL: $path → code 不是 0"; fail=1; continue
  fi
  echo "PASS: $path"
done

echo "== 退出清理（SIGTERM 必须被处理） =="
kill -TERM "$daemon_pid"
wait "$daemon_pid" 2>/dev/null || true
daemon_pid=""
sleep 1
if [ -f "$work_dir/runtime.json" ]; then
  echo "FAIL: 退出后 runtime.json 还在（前端下次启动会读到过期端口）"; fail=1
else
  echo "PASS: SIGTERM 后清理了 runtime.json"
fi

if [ "$fail" -ne 0 ]; then
  echo; echo "打包产物校验失败。daemon 日志："; tail -20 "$log_file"; exit 1
fi

echo
echo "打包产物校验通过（$app 里的 daemon 可用）。"
