#!/usr/bin/env bash
# 性能基线：启动耗时与空闲内存。
#
# 为什么单独一个脚本而不是 cargo 测试：这两项测的是**进程级**行为
# （冷启动到 health 就绪、静置后的 RSS），用 cargo 测反而要在测试里
# 起进程再采样，慢且不稳。脚本直接测真实二进制，也就是用户实际拿到的东西。
#
# 数值会随机器与负载剧烈变化，因此**每次运行都把环境一起打出来**
# （`sw_vers`、负载、是否 release），并把阈值留得宽松 ——
# 它用来发现「数量级变化」，不是做微基准。
#
# 用法：scripts/bench-baseline.sh [--runs 5] [--idle 5]
set -uo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

RUNS=5
IDLE=5
while [ $# -gt 0 ]; do
  case "$1" in
    --runs) RUNS="${2:-5}"; shift 2 ;;
    --idle) IDLE="${2:-5}"; shift 2 ;;
    *) echo "未知参数：$1"; exit 2 ;;
  esac
done

echo "== 环境 =="
sw_vers | tr '\n' ' '; echo
echo "load: $(uptime | sed 's/.*load averages*: //')"
echo "构建：release（测量真实产物）"
echo

echo "== 构建 release 二进制 =="
if ! cargo build --release -p mc-cli -p mc-daemon >/tmp/bench-build.log 2>&1; then
  echo "构建失败，见 /tmp/bench-build.log"; exit 1
fi

CLI=target/release/mc-cli
DAEMON=target/release/mc-daemon
[ -x "$CLI" ] || { echo "缺少 $CLI"; exit 1; }
[ -x "$DAEMON" ] || { echo "缺少 $DAEMON"; exit 1; }

# ---------------------------------------------------------------- CLI 冷启动
echo "== mc-cli doctor 冷启动（${RUNS} 次，毫秒） =="
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

times=()
for _ in $(seq "$RUNS"); do
  data_dir="$work/cli-$RANDOM"
  mkdir -p "$data_dir"
  start=$(xtask_run now-ms)
  "$CLI" doctor --data-dir "$data_dir" >/dev/null 2>&1
  end=$(xtask_run now-ms)
  times+=($((end - start)))
done

xtask_run percentiles "${times[@]}"
echo

# ---------------------------------------------------------------- daemon 就绪
echo "== daemon 冷启动到 /api/health 就绪 =="
data_dir="$work/daemon"
mkdir -p "$data_dir"
#"$DAEMON" 会把端口与 token 写进 runtime.json
"$DAEMON" --data-dir "$data_dir" >/tmp/bench-daemon.log 2>&1 &
daemon_pid=$!

ready_ms=""
start=$(xtask_run now-ms)
for _ in $(seq 200); do
  if [ -f "$data_dir/runtime.json" ]; then
    port=$(xtask_run json-field "$data_dir/runtime.json" port 2>/dev/null)
    token=$(xtask_run json-field "$data_dir/runtime.json" token 2>/dev/null)
    if [ -n "${port:-}" ] && curl -sf -m 1 -H "x-mc-token: $token" \
        "http://127.0.0.1:$port/api/health" >/dev/null 2>&1; then
      end=$(xtask_run now-ms)
      ready_ms=$((end - start))
      break
    fi
  fi
  sleep 0.05
done

if [ -n "$ready_ms" ]; then
  echo "  就绪耗时 ${ready_ms}ms（pid ${daemon_pid}，端口 ${port:-?}）"
else
  echo "  未就绪：见 /tmp/bench-daemon.log"
fi

# ---------------------------------------------------------------- 空闲内存
if [ -n "$ready_ms" ]; then
  echo "== 空闲 ${IDLE}s 后的常驻内存 =="
  sleep "$IDLE"
  rss_kb=$(ps -o rss= -p "$daemon_pid" 2>/dev/null | tr -d ' ')
  if [ -n "$rss_kb" ]; then
    awk -v kb="$rss_kb" 'BEGIN { printf "  RSS %.1f MB\n", kb / 1024 }'
  else
    echo "  进程已退出，无法采样"
  fi

  # 诊断接口的字段齐全性顺手一起看（可观测性回归）
  diag=$(curl -sf -m 2 -H "x-mc-token: $token" "http://127.0.0.1:$port/api/diagnostics")
  xtask_run diag-fields "$diag" 2>/dev/null || echo "  诊断接口未返回可解析 JSON"
fi

kill "$daemon_pid" 2>/dev/null
wait "$daemon_pid" 2>/dev/null
echo
echo "基线数值请记录到 docs/decisions/performance-baseline.md（连同上面的环境信息）。"
