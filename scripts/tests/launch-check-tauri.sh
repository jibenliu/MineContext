#!/usr/bin/env bash
# Tauri 外壳的产物启动检查。
#
# 产物是 **dmg 里的 .app**，因此这里要先挂载，验六件事：
#   1. dmg 挂载成功，且 `Contents/Resources/backend/mc-daemon` 在包里（可执行）；
#   2. 用临时数据目录启动后，daemon 写出 `runtime.json`（窗口拿得到端口）；
#   3. SIGTERM 之后 `runtime.json` 与 `.shell.lock` 都被清掉（不留过期端口）；
#   4. 卸载 dmg、清理临时目录与进程。
#
# 前置：`cd src-tauri && cargo tauri build --debug`（或 release）。
set -uo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

dmg="${1:-$(ls -t src-tauri/target/debug/bundle/dmg/*.dmg src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)}"
if [ -z "${dmg}" ]; then
  echo "SKIP: 还没有 Tauri 产物。先跑：cd src-tauri && cargo tauri build --debug"
  exit 0
fi

mount_point="$(mktemp -d)"
work_dir="$(mktemp -d)"
app_pid=""

cleanup() {
  status=$?
  if [ -n "${app_pid}" ] && kill -0 "${app_pid}" 2>/dev/null; then
    kill -TERM "${app_pid}" 2>/dev/null || true
    for _ in $(seq 1 20); do
      if ! kill -0 "${app_pid}" 2>/dev/null; then break; fi
      sleep 0.1
    done
    kill -KILL "${app_pid}" 2>/dev/null || true
  fi
  if [ -n "${app_pid}" ]; then
    wait "${app_pid}" 2>/dev/null || true
  fi
  # 先卸载再删目录：反过来会撞上「Read-only file system」（挂载点还在用）
  hdiutil detach "${mount_point}" -force -quiet 2>/dev/null || true
  rmdir "${mount_point}" 2>/dev/null || true
  if [ "$status" -eq 0 ]; then
    rm -rf "${work_dir}"
  else
    echo "诊断目录已保留（可能含敏感信息）：${work_dir}"
    for logfile in "${work_dir}/shell.log" "${work_dir}/logs/daemon.log"; do
      if [ -f "$logfile" ]; then
        echo "== ${logfile} =="
        tail -30 "$logfile"
      fi
    done
  fi
  return "$status"
}
trap cleanup EXIT

# 同一个 dmg 可能因为上一次异常退出仍挂着：先卸掉，避免「挂载失败」这种噪音
stale="$(hdiutil info | awk -v image="${dmg}" '$0 ~ image {found=1} found && /^\/dev\/disk/ {print $1; exit}')"
if [ -n "${stale}" ]; then
  hdiutil detach "${stale}" -force -quiet 2>/dev/null || true
fi

echo "== 挂载 ${dmg} =="
if ! hdiutil attach "${dmg}" -mountpoint "${mount_point}" -nobrowse -quiet; then
  echo "FAIL: dmg 挂载失败"
  exit 1
fi

app="${mount_point}/MineContext.app"
daemon="${app}/Contents/Resources/backend/mc-daemon"
binary="$(ls "${app}/Contents/MacOS/" 2>/dev/null | head -1 || true)"

fail=0
if [ -x "${daemon}" ]; then
  echo "PASS: 包里有可执行的 daemon（Contents/Resources/backend/mc-daemon）"
else
  echo "FAIL: 包里没有可执行的 daemon"; fail=1
fi

if [ -n "${binary}" ] && [ -x "${app}/Contents/MacOS/${binary}" ]; then
  echo "PASS: 找到外壳可执行文件（${binary}）"
else
  echo "FAIL: 找不到外壳可执行文件"; fail=1
fi

if [ "${fail}" -ne 0 ]; then
  echo "产物不完整，跳过启动检查"
  exit 1
fi

echo "== 启动（临时数据目录） =="
MC_DATA_DIR="${work_dir}" "${app}/Contents/MacOS/${binary}" >"${work_dir}/shell.log" 2>&1 &
app_pid=$!

for _ in $(seq 1 40); do
  if [ -f "${work_dir}/runtime.json" ]; then break; fi
  if ! kill -0 "${app_pid}" 2>/dev/null; then
    echo "FAIL: 外壳提前退出，日志："; tail -10 "${work_dir}/shell.log"; exit 1
  fi
  sleep 0.5
done

if [ -f "${work_dir}/runtime.json" ]; then
  port="$(xtask_run json-field "${work_dir}/runtime.json" port)"
  echo "PASS: daemon 写出 runtime.json（port ${port}）"
else
  echo "FAIL: 20 秒内没有 runtime.json"; tail -10 "${work_dir}/shell.log"; exit 1
fi

# 渲染层真的起来了才算「外壳可用」：只验 daemon 的话，`window.__TAURI__` 没注入
# （`withGlobalTauri` 忘了开）这种问题会表现为**打包版白屏而检查全绿**，所以必须真启动一次。
# 判据是渲染层 bootstrap 写出的自检日志，它经 `renderer_log` 命令进外壳 stdout。
echo "== 渲染层 bootstrap =="
renderer_up=0
for _ in $(seq 1 40); do
  if grep -q "适配层已装" "${work_dir}/shell.log"; then renderer_up=1; break; fi
  if ! kill -0 "${app_pid}" 2>/dev/null; then break; fi
  sleep 0.5
done

if [ "${renderer_up}" -eq 1 ]; then
  echo "PASS: 渲染层完成 bootstrap（适配层已装，日志经由外壳落盘）"
else
  echo "FAIL: 20 秒内没看到渲染层的启动自检（打包版可能是白屏）"
  echo "      先查 window.__TAURI__ 是否存在（tauri.conf.json 的 app.withGlobalTauri）"
  fail=1
fi

echo "== SIGTERM 清理 =="
kill -TERM "${app_pid}" 2>/dev/null || true
for _ in $(seq 1 20); do
  if [ ! -f "${work_dir}/runtime.json" ]; then break; fi
  sleep 0.5
done

if [ -f "${work_dir}/runtime.json" ]; then
  echo "FAIL: SIGTERM 后 runtime.json 还在（下次会读到过期端口）"; fail=1
else
  echo "PASS: SIGTERM 后清掉了 runtime.json"
fi
if [ -f "${work_dir}/.shell.lock" ]; then
  echo "FAIL: SIGTERM 后 .shell.lock 还在（下次启动会被误判为已在运行）"; fail=1
else
  echo "PASS: SIGTERM 后清掉了 .shell.lock"
fi

# 文件清掉了不等于进程退出了：实测过「文件已清、进程仍在，而且以 200% CPU 空转」
# 的情况（信号线程里直接 std::process::exit 会与 AppKit 的拆除抢主线程）。
exited=0
for _ in $(seq 1 20); do
  if ! kill -0 "${app_pid}" 2>/dev/null; then exited=1; break; fi
  sleep 0.5
done
if [ "${exited}" -eq 1 ]; then
  echo "PASS: SIGTERM 后外壳进程真的退出了"
else
  echo "FAIL: SIGTERM 后外壳进程还在（残留进程会一直占 CPU，且下次启动会撞单实例锁）"
  kill -KILL "${app_pid}" 2>/dev/null || true
  fail=1
fi
wait "${app_pid}" 2>/dev/null || true
app_pid=""

if [ "$fail" -ne 0 ]; then
  echo; echo "Tauri 启动检查失败。外壳日志："; tail -20 "${work_dir}/shell.log"; exit 1
fi

echo
echo "Tauri 启动检查通过（dmg → 挂载 → 启动 → 渲染层 bootstrap → daemon 就绪 → 退出清理 → 进程真的退出）。"
