#!/usr/bin/env bash
# 真进程冒烟：跑**真的 daemon 二进制**，用它写的 `runtime.json` 访问 API。
#
# 为什么值得一个脚本：适配层测试、页面级测试、甚至 `cargo test --workspace`
# 都不能回答「交付出去的 daemon 到底能不能被前端用起来」。这里把那条链路
# 端到端跑一遍：
#
#   mc-daemon（真进程）→ runtime.json → 带 token 的 HTTP 请求 → 响应形状
#
# 断言四类东西：
#   1. 每个前端会调的接口都返回 `code=0` 且形状对得上；
#   2. **没有 token 一律 401**（安全属性在真进程里也成立）；
#   3. 响应里不出现 token、也不出现数据目录绝对路径；
#   4. 退出时清理 `runtime.json`（否则前端会读到过期端口）。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

port_file="$(mktemp)"
work_dir="$(mktemp -d)"
log_file="$(mktemp)"
runtime=""
daemon_pid=""

cleanup() {
  if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf "$work_dir" "$port_file" "$log_file"
}
trap cleanup EXIT

echo "== 构建 daemon =="
cargo build -p mc-daemon --bin mc-daemon >/dev/null 2>&1
binary="target/debug/mc-daemon"
[ -x "$binary" ] || { echo "FAIL: 没有构建出 $binary"; exit 1; }

# 采集先关掉：冒烟不该触发屏幕录制权限弹窗
cat > "$work_dir/config.toml" <<'TOML'
[capture]
enabled = false
TOML

echo "== 起 daemon（数据目录 ${work_dir}） =="
"$binary" --data-dir "$work_dir" --config "$work_dir/config.toml" --port 0 \
  >"$log_file" 2>&1 &
daemon_pid=$!

# 等 runtime.json：daemon 起来后才写，写出来才说明真的在监听
for _ in $(seq 1 100); do
  if [ -f "$work_dir/runtime.json" ]; then break; fi
  if ! kill -0 "$daemon_pid" 2>/dev/null; then
    echo "FAIL: daemon 提前退出，日志："
    tail -20 "$log_file"
    exit 1
  fi
  sleep 0.2
done

if [ ! -f "$work_dir/runtime.json" ]; then
  echo "FAIL: 20 秒内没有写出 runtime.json，日志："
  tail -20 "$log_file"
  exit 1
fi

port="$(xtask_run json-field "$work_dir/runtime.json" port)"
token="$(xtask_run json-field "$work_dir/runtime.json" token)"
base="http://127.0.0.1:${port}"
echo "   port=${port}（runtime.json 已就绪）"

fail=0

# 请求：$1 方法 $2 路径 [$3 body]；输出「HTTP 状态码<TAB>响应体」
request() {
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -s -o "$port_file" -w '%{http_code}' -X "$method" \
      -H "x-mc-token: $token" -H 'content-type: application/json' \
      -d "$body" "${base}${path}"
  else
    curl -s -o "$port_file" -w '%{http_code}' -X "$method" \
      -H "x-mc-token: $token" "${base}${path}"
  fi
}

# 断言：$1 说明 $2 方法 $3 路径 $4 期望字段 $5 请求体（可选）
check_ok() {
  local what="$1" method="$2" path="$3" needle="$4" payload="${5:-}"
  local status body
  status="$(request "$method" "$path" "$payload")"
  body="$(cat "$port_file")"

  if [ "$status" != "200" ]; then
    echo "FAIL: $what → HTTP $status"; echo "      ${body:0:200}"; fail=1; return
  fi
  if ! printf '%s' "$body" | grep -q '"code":0'; then
    echo "FAIL: $what → code 不是 0"; echo "      ${body:0:200}"; fail=1; return
  fi
  if [ -n "$needle" ] && ! printf '%s' "$body" | grep -q "$needle"; then
    echo "FAIL: $what → 响应里没有 $needle"; echo "      ${body:0:200}"; fail=1; return
  fi
  # 任何响应都不许带 token 或数据目录绝对路径
  if printf '%s' "$body" | grep -q "$token"; then
    echo "FAIL: $what → 响应里出现了 token"; fail=1; return
  fi
  if printf '%s' "$body" | grep -q "$work_dir"; then
    echo "FAIL: $what → 响应里出现了数据目录路径"; fail=1; return
  fi
  echo "PASS: $what"
}

echo "== 前端会调的接口 =="
check_ok "GET /api/diagnostics" GET "/api/diagnostics" '"invariants"'
check_ok "GET /api/v1/diagnostics/export" GET "/api/v1/diagnostics/export" '"schema_version"'
check_ok "GET /api/model_settings/get" GET "/api/model_settings/get" '"config"'
check_ok "GET /api/capture/status" GET "/api/capture/status" ''
check_ok "GET /api/capture/targets" GET "/api/capture/targets" ''
check_ok "GET /api/capture/config" GET "/api/capture/config" ''
check_ok "GET /api/db/activities" GET "/api/db/activities" ''
check_ok "GET /api/db/vaults" GET "/api/db/vaults" ''
check_ok "GET /api/db/todos" GET "/api/db/todos" ''
check_ok "GET /api/db/tips" GET "/api/db/tips" ''
check_ok "GET /api/db/heatmap" GET "/api/db/heatmap?start=0&end=1" ''
check_ok "GET /api/v1/summaries" GET "/api/v1/summaries" ''
check_ok "GET /api/v1/threads" GET "/api/v1/threads" ''
check_ok "GET /api/agent/chat/conversations/list" GET "/api/agent/chat/conversations/list" ''
check_ok "GET /api/monitoring/recording-stats" GET "/api/monitoring/recording-stats" ''
# 形状校验：合法配置 → valid=true（只验形状，不调模型）
check_ok "POST /api/model_settings/validate" POST "/api/model_settings/validate" \
  '"valid":true' \
  '{"config":{"modelId":"m","embeddingModelId":"e","baseUrl":"https://example.com/v1"}}'
check_ok "POST /api/capture/now（无权限时应报采集错误）" POST "/api/capture/now" ''

echo "== 对话写入链路（旧前端的会话服务走的就是这几条） =="
# create → get → delete（软删）。会话服务切到 rust 后端后仍用 axios 打这些路径，
# 因此它们必须真的可用，而不只是「注册了路由、返回未实现」。
status="$(request POST "/api/agent/chat/conversations" '{"page_name":"home"}')"
cid="$(xtask_run json-field "$port_file" data.id 2>/dev/null || echo "")"
if [ -n "$cid" ]; then
  echo "PASS: POST /api/agent/chat/conversations → id=${cid}"
else
  echo "FAIL: 创建会话失败"; cat "$port_file"; fail=1; cid=""
fi

check_ok "GET /api/agent/chat/conversations/list" GET "/api/agent/chat/conversations/list" ''

if [ -n "$cid" ]; then
  check_ok "GET /api/agent/chat/conversations/{cid}" GET "/api/agent/chat/conversations/${cid}" '"id"'
  check_ok "POST message/{mid}/create" POST "/api/agent/chat/message/parent-1/create" '"code":0' \
    "{\"conversation_id\":${cid},\"role\":\"user\",\"content\":\"冒烟提问\",\"is_complete\":true}"
  check_ok "DELETE conversations/{cid}/update（软删）" DELETE "/api/agent/chat/conversations/${cid}/update" ''
fi

echo "== 鉴权：没有 token 必须 401 =="
for path in /api/diagnostics /api/v1/diagnostics/export /api/model_settings/get /api/db/vaults; do
  status="$(curl -s -o "$port_file" -w '%{http_code}' "${base}${path}")"
  if [ "$status" = "401" ]; then
    echo "PASS: 无 token 访问 $path → 401"
  else
    echo "FAIL: 无 token 访问 $path → HTTP ${status}（必须 401）"; fail=1
  fi
done

echo "== 公开面：健康检查不需要 token =="
status="$(curl -s -o "$port_file" -w '%{http_code}' "${base}/api/health")"
if [ "$status" = "200" ]; then echo "PASS: GET /api/health 公开"; else
  echo "FAIL: GET /api/health → HTTP $status"; fail=1
fi

echo "== 运行日志（落盘 / 可定位 / 不含内容、密钥、路径） =="
engine_log="${work_dir}/logs/daemon.log"
if [ ! -f "$engine_log" ]; then
  echo "FAIL: 没有写出运行日志 ${engine_log}"; fail=1
else
  if grep -q 'event="listening"' "$engine_log"; then
    echo "PASS: 日志里有监听事件"
  else
    echo "FAIL: 日志里没有 listening 事件"; tail -5 "$engine_log"; fail=1
  fi
  if grep -q 'component="daemon"' "$engine_log"; then
    echo "PASS: 日志带 component 字段"
  else
    echo "FAIL: 日志缺 component 字段（定位不到来源）"; fail=1
  fi
  leaked=""
  for needle in "${token}" "${work_dir}" "${HOME}"; do
    if grep -q -F -- "${needle}" "$engine_log"; then leaked="${needle}"; fi
  done
  if [ -n "$leaked" ]; then
    echo "FAIL: 日志里出现了不该有的内容（密钥或路径）"; fail=1
  else
    echo "PASS: 日志不含密钥与绝对路径"
  fi
  if grep -q -F "冒烟提问" "$engine_log"; then
    echo "FAIL: 日志里出现了消息内容"; fail=1
  else
    echo "PASS: 日志不含消息内容"
  fi
  if grep -q -F "$(printf '\033')" "$engine_log"; then
    echo "FAIL: 日志里有终端转义序列（支持包里没法 grep）"; fail=1
  else
    echo "PASS: 日志是纯文本"
  fi
fi

echo "== 退出清理 =="
kill "$daemon_pid" 2>/dev/null || true
wait "$daemon_pid" 2>/dev/null || true
daemon_pid=""
sleep 0.5
if [ -f "$work_dir/runtime.json" ]; then
  echo "FAIL: 退出后 runtime.json 还在（前端会读到过期端口）"; fail=1
else
  echo "PASS: 退出时清理了 runtime.json"
fi

if [ "$fail" -ne 0 ]; then
  echo
  echo "真进程冒烟失败。daemon 日志："
  tail -30 "$log_file"
  exit 1
fi

echo
echo "真进程冒烟通过（daemon 真起的，响应形状与鉴权都验过）。"
