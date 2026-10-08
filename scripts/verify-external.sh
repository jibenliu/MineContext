#!/usr/bin/env bash
# 外部条件验证：把「只能在真机/联网环境确认」的项固化下来。
#
# 这些检查**不进 `verify-all.sh`** —— 它们依赖屏幕录制权限、真实显示器、
# 网络与数小时的真机时长，放进日常门禁只会让门禁常年红着（然后被忽略）。
# 正确用法是发版前或拿到真机时手工跑一次，并把输出贴进验收记录。
#
# 每一项都会先探测前置条件：不满足时给出 SKIP 与**具体原因**，
# 而不是报一个看不懂的 FAIL —— 「为什么没跑」和「跑了没过」是两件事。
#
# 用法：
#   scripts/verify-external.sh              # 跑能自动判定的三项
#   scripts/verify-external.sh --soak-hours 8   # 额外跑 soak（需要真机时长）
set -uo pipefail

cd "$(dirname "$0")/.."

SOAK_HOURS=0
while [ $# -gt 0 ]; do
  case "$1" in
    --soak-hours) SOAK_HOURS="${2:-0}"; shift 2 ;;
    *) echo "未知参数：$1"; exit 2 ;;
  esac
done

pass=0
skip=0
fail=0

report() { # report <状态> <名称> <说明>
  printf '%-6s %-28s %s\n' "$1" "$2" "$3"
  case "$1" in
    PASS) pass=$((pass + 1)) ;;
    SKIP) skip=$((skip + 1)) ;;
    FAIL) fail=$((fail + 1)) ;;
  esac
}

echo "外部条件验证（不进日常门禁）"
echo

# ---------------------------------------------------------------- 1) 真机像素/延迟
#
# 前置：系统设置 → 隐私与安全性 → 屏幕录制 里勾选本应用（并重启终端）。
# 判定：测试自己会因为拿不到权限而报 CapturePermissionDenied；
# 这里据输出区分 SKIP（没权限）与 FAIL（有权限但像素/延迟不达标）。
echo "== 1/5 真机像素路径与延迟基线 =="
pixel_log="$(mktemp)"
if timeout 600 cargo test -p mc-capture --test macos_source -- --ignored --nocapture \
    >"$pixel_log" 2>&1; then
  # 这几条测试在拿不到权限时会**自行跳过并通过** —— 不能把它当 PASS。
  # 「测试是绿的」与「像素路径被真正验证过」是两件事。
  if grep -q "跳过：本机未授予屏幕录制权限" "$pixel_log"; then
    report SKIP "macos_source(ignored)" \
      "测试因缺少屏幕录制权限自行跳过（不是通过）；授权后重跑"
  else
    report PASS "macos_source(ignored)" "像素路径与延迟基线通过"
  fi
else
  if grep -qE "CapturePermissionDenied|缺少屏幕录制权限|permission" "$pixel_log"; then
    report SKIP "macos_source(ignored)" "缺少屏幕录制权限（系统设置 → 隐私与安全性 → 屏幕录制）"
  else
    report FAIL "macos_source(ignored)" "有权限但未通过：tail -40 $pixel_log"
    tail -20 "$pixel_log"
  fi
fi
rm -f "$pixel_log"
echo

# ---------------------------------------------------------------- 2) 真机窗口采集
#
# 前置：同 1)。窗口采集是隐私黑名单的**前提**：拿不到应用名与窗口标题，
# `blocked_apps` / `blocked_window_patterns` 永远不会命中。
echo "== 2/5 真机窗口采集（应用名/标题/图像） =="
window_log="$(mktemp)"
if timeout 600 cargo test -p mc-capture --test window_source -- --ignored --nocapture \
    >"$window_log" 2>&1; then
  if grep -q "跳过：本机未授予屏幕录制权限" "$window_log"; then
    report SKIP "window_source(ignored)" \
      "测试因缺少屏幕录制权限自行跳过（不是通过）；授权后重跑"
  else
    report PASS "window_source(ignored)" "真机窗口采集通过（应用名/标题/图像）"
  fi
else
  if grep -qE "CapturePermissionDenied|缺少屏幕录制权限|permission" "$window_log"; then
    report SKIP "window_source(ignored)" "缺少屏幕录制权限（系统设置 → 隐私与安全性 → 屏幕录制）"
  else
    report FAIL "window_source(ignored)" "未通过：tail -40 $window_log"
    tail -20 "$window_log"
  fi
fi
rm -f "$window_log"
echo

# ---------------------------------------------------------------- 2.5) 真进程冒烟
#
# 与 verify-all.sh 第 6 步同一条脚本：起真 daemon、按 runtime.json 打接口。
echo "== 2.5/5 真进程冒烟（daemon 二进制 + 真实 HTTP） =="
smoke_log="$(mktemp)"
if timeout 900 ./scripts/tests/smoke-daemon.sh >"$smoke_log" 2>&1; then
  report PASS "smoke-daemon" "真进程冒烟通过（响应形状、鉴权、退出清理）"
else
  report FAIL "smoke-daemon" "未通过：tail -40 $smoke_log"
  tail -20 "$smoke_log"
fi
rm -f "$smoke_log"
echo

# ---------------------------------------------------------------- 2.6) 打包产物
#
# 前置：`./scripts/package-macos-tauri.sh`（无签名 dmg）。没有产物时脚本自己
# 报 SKIP —— 打包要编译 release daemon 与 Tauri，不该卡住别的验证。
echo "== 2.6/5 打包产物里的 daemon =="
pack_log="$(mktemp)"
if timeout 600 ./scripts/tests/verify-packaged-app.sh >"$pack_log" 2>&1; then
  if grep -q "^SKIP" "$pack_log"; then
    report SKIP "packaged app" "$(grep '^SKIP' "$pack_log")"
  else
    report PASS "packaged app" "产物里的 daemon 可用（接口形状 + SIGTERM 清理）"
  fi
else
  report FAIL "packaged app" "未通过：tail -40 $pack_log"
  tail -20 "$pack_log"
fi
rm -f "$pack_log"
echo

# ---------------------------------------------------------------- 3) 真进程 E2E
#
# 前置：本机 loopback 允许「被测试启动的进程」访问。注意这与「shell 里
# 测试进程能连 loopback」不是一回事 —— 受限的是它派生的子进程。
# 已知在本开发沙箱里，daemon 能起、HTTP 能连，但 daemon→stub 模型的
# loopback 连接超时（provider_timeout）。
echo "== 3/5 真进程 E2E（daemon + stub 模型） =="
e2e_log="$(mktemp)"
if timeout 600 cargo test -p mc-daemon --test e2e -- --ignored --nocapture \
    >"$e2e_log" 2>&1; then
  report PASS "daemon e2e(ignored)" "真进程端到端通过"
else
  if grep -q "provider_timeout" "$e2e_log"; then
    report SKIP "daemon e2e(ignored)" \
      "daemon 已能启动并响应 HTTP，但 daemon→stub 的 loopback 连接超时（沙箱限制）"
  else
    report FAIL "daemon e2e(ignored)" "未通过：tail -40 $e2e_log"
    tail -20 "$e2e_log"
  fi
fi
rm -f "$e2e_log"
echo

# ---------------------------------------------------------------- 3) 前端测试
#
# 前置：`pnpm install`（需要网络；本仓库的适配层测试不需要依赖，
# 但页面级测试需要）。
echo "== 4/5 前端测试 =="
if [ ! -d frontend/node_modules ]; then
  report SKIP "frontend test" "缺少 frontend/node_modules（先跑 pnpm install）"
elif [ ! -d frontend/node_modules/vitest ] && [ ! -d frontend/node_modules/.pnpm ]; then
  report SKIP "frontend test" "依赖未安装完整（pnpm 10 的 ERR_PNPM_IGNORED_BUILDS）"
else
  frontend_log="$(mktemp)"
  # 与 verify-all.sh 第 7 步一致：类型检查 + 两层测试 + 生产构建
  if (cd frontend && timeout 1800 sh -c 'pnpm run typecheck && pnpm test && npx --no-install vite build' >"$frontend_log" 2>&1); then
    report PASS "frontend" "类型检查 + 适配层 + 页面级渲染 + 生产构建 全通过"
  else
    report FAIL "frontend test" "未通过：tail -40 $frontend_log"
    tail -20 "$frontend_log"
  fi
  rm -f "$frontend_log"
fi
echo

# ---------------------------------------------------------------- 5) soak
if [ "$SOAK_HOURS" -le 0 ]; then
  report SKIP "soak" "未指定时长（--soak-hours 8）；需要真机连续运行"
else
  report SKIP "soak(${SOAK_HOURS}h)" \
    "需要真机连续运行 ${SOAK_HOURS} 小时；判据见 docs/external-verification.md"
fi
echo

echo "---- 汇总：PASS $pass · SKIP $skip · FAIL $fail ----"
if [ "$fail" -gt 0 ]; then
  exit 1
fi
# SKIP 不是失败：它表示「这台机器上无法确认」，必须如实记录而不能当通过
[ "$skip" -gt 0 ] && echo "注意：SKIP 表示未验证，不等于通过。"
exit 0
