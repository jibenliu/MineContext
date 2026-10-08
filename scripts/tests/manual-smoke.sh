#!/usr/bin/env bash
# 真机人工确认的辅助脚本：把「要点什么、该看到什么」列出来，挂上 dmg 把应用拉起来，
# 并**逐项记录**结果（写进本机留档目录；该目录不入库）。
#
# 为什么单独一个脚本：`launch-check-tauri.sh` 能验「外壳起来、渲染层 bootstrap、
# 文件清理、进程退出」，但**托盘点击、系统通知弹窗、关窗收进托盘、开机自启**这些
# 只能人点人看。与其在文档里写一句「待人工确认」，不如把步骤固化、让结果可记录。
set -uo pipefail

cd "$(dirname "$0")/../.."

launch=0
record=""
for arg in "$@"; do
  case "${arg}" in
    --launch) launch=1 ;;
    --record) record=1 ;;
    --help|-h)
      echo "用法：$0 [--launch] [--record]"
      echo "  --launch  挂载 dmg 并用临时数据目录把应用拉起来（不碰现有数据）"
      echo "  --record  逐项询问结果（y/n/skip + 备注），写进 docs/internal/manual-smoke-<日期>.md"
      exit 0
      ;;
  esac
done

dmg="$(ls -t src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)"

cat <<'CHECKLIST'
== 真机人工确认清单（Tauri 外壳） ==

前置：`./scripts/package-macos-tauri.sh` 出 dmg（未签名，首次打开需右键「打开」）。

A. 已经自动化验证过的（这里只列出判据，不必人点；复验跑 `./scripts/tests/launch-check-tauri.sh`）
   A1 窗口正常显示，渲染层画出真实界面（不是白屏、不停在加载页）
   A2 daemon 已就绪（界面能正常取数，控制台没有 401）
   A3 `pkill -TERM` 后 `runtime.json` / `.shell.lock` 被清掉，**且进程真的退出**

B. 必须人点人看的
   B1 托盘菜单「显示窗口」：先关窗（应收进托盘），再点它 → 窗口回来
   B2 托盘菜单「开始 / 暂停录制」：点一次 → 应显示窗口、跳到屏幕监控页并切换采集状态；
      托盘悬停提示与菜单文案应随之变化（文案只在屏幕监控页挂载时更新，页面关掉后可能偏旧）
   B3 托盘菜单「屏幕监控」：点一次 → 窗口显示并跳到该页
   B4 托盘菜单「退出」：应用退出；`ls "<数据目录>/runtime.json"` 应不存在，无残留 daemon
   B5 关窗收进托盘：关窗后应用仍在（Dock/托盘可见），后台继续采集（屏幕监控页有记录增长）
   B6 系统通知真的弹出：设置页打开通知开关，触发一次 → 通知中心出现该通知
   B7 开机自启：设置页勾选 → `launchctl list | grep -i minecontext` 有 LaunchAgent；
      注销再登录后应用自动启动（取消勾选后不再启动）
   B8 界面内交互：搜索页输入关键词回车有命中/明确空态；助手页逐块出字；
      设置页能读出当前配置

C. 记录与登记
   把结果写进 `docs/internal/manual-smoke-<日期>.md`（--record 会代劳），
   并在 `docs/operations.md` §7 更新对应行的状态（不能因为"代码写了"就记成完成）。

CHECKLIST

if [ -z "${dmg}" ]; then
  echo "SKIP: 还没有 Tauri 产物。先跑：./scripts/package-macos-tauri.sh"
  exit 0
fi

echo "产物：${dmg}"
echo

if [ "${launch}" -ne 1 ]; then
  echo "（只打印清单。加 --launch 会挂载 dmg 并拉起应用；加 --record 会逐项记录结果。）"
  exit 0
fi

mount_point="$(mktemp -d)"
work_dir="$(mktemp -d)"
app_pid=""

cleanup() {
  if [ -n "${app_pid}" ] && kill -0 "${app_pid}" 2>/dev/null; then
    kill -TERM "${app_pid}" 2>/dev/null || true
  fi
  pkill -f "${mount_point}/MineContext.app/Contents/MacOS/" 2>/dev/null || true
  hdiutil detach "${mount_point}" -force -quiet 2>/dev/null || true
  rm -rf "${mount_point}" "${work_dir}" 2>/dev/null || true
}
trap cleanup EXIT

hdiutil detach "${mount_point}" -force -quiet 2>/dev/null || true
if ! hdiutil attach "${dmg}" -mountpoint "${mount_point}" -nobrowse -quiet; then
  echo "FAIL: dmg 挂载失败"
  exit 1
fi

app="${mount_point}/MineContext.app"
binary="$(ls "${app}/Contents/MacOS/" 2>/dev/null | head -1 || true)"
if [ -z "${binary}" ]; then
  echo "FAIL: 包结构不对，找不到可执行文件"
  exit 1
fi

echo "== 拉起应用（数据目录 ${work_dir}，不碰你现有的数据） =="
MC_DATA_DIR="${work_dir}" "${app}/Contents/MacOS/${binary}" >"${work_dir}/shell.log" 2>&1 &
app_pid=$!

sleep 12
if ! kill -0 "${app_pid}" 2>/dev/null; then
  echo "FAIL: 应用提前退出，日志："
  tail -20 "${work_dir}/shell.log"
  exit 1
fi
echo "应用在运行（pid ${app_pid}）。daemon 进程："
pgrep -fl "Resources/backend/mc-daemon" || echo "  （没看到 daemon —— 这是缺陷，请记录）"
echo

if [ "${record}" -ne 1 ]; then
  echo "确认完在界面上退出应用即可；本脚本结束时也会收掉它。"
  wait "${app_pid}" 2>/dev/null || true
  exit 0
fi

# ---- 逐项记录 ----
out="docs/internal/manual-smoke-$(date +%Y%m%d-%H%M).md"
{
  echo "# 真机人工确认记录（$(date '+%Y-%m-%d %H:%M')）"
  echo
  echo "- 产物：\`${dmg}\`"
  echo "- 机器：$(sw_vers -productVersion) / $(uname -m)"
  echo "- 方式：\`scripts/tests/manual-smoke.sh --launch --record\`（临时数据目录，未触碰现有数据）"
  echo
  echo "| 项 | 结果 | 现象 / 备注 |"
  echo "|---|---|---|"
} >"${out}"

ask() {
  local item="$1" prompt="$2" verdict note
  printf '%s（y=通过 / n=不通过 / s=跳过）：' "${prompt}"
  read -r verdict || verdict="s"
  case "${verdict}" in
    y|Y) verdict="通过" ;;
    n|N) verdict="**不通过**" ;;
    *) verdict="未确认" ;;
  esac
  printf '  现象 / 备注（可空）：'
  read -r note || note=""
  echo "| ${item} | ${verdict} | ${note} |" >>"${out}"
}

ask "B1 托盘·显示窗口" "关窗后点托盘「显示窗口」，窗口回来了吗？"
ask "B2 托盘·开始/暂停录制" "点托盘「开始 / 暂停录制」，是否显示窗口+跳到屏幕监控页+切换了采集？"
ask "B3 托盘·屏幕监控" "点托盘「屏幕监控」，是否显示窗口并跳到该页？"
ask "B4 托盘·退出" "点托盘「退出」，应用是否退出且无残留进程？"
ask "B5 关窗收进托盘" "关窗后应用是否仍在运行且继续采集？"
ask "B6 系统通知" "触发通知后，macOS 通知中心是否真的弹出？"
ask "B7 开机自启" "勾选后 LaunchAgent 存在、（注销重登）应用自动启动？"
ask "B8 界面交互" "搜索 / 助手 / 设置 三个页面是否都正常？"

{
  echo
  echo "记录人：（请填）"
} >>"${out}"

echo
echo "结果已写入 ${out}"
echo "请把不通过的项同步到 docs/operations.md §7 的状态里（未验证 ≠ 通过）。"

wait "${app_pid}" 2>/dev/null || true
