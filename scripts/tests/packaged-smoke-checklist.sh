#!/usr/bin/env bash
# 发版前「打包产物冒烟」清单：固化打包/安装产物、daemon/渲染层健康、录制开关、
# 助手一条消息、文件或链接摄入一条路径。与 manual-smoke.sh（托盘/通知/自启观感）
# 互补；**不**接入日常 verify——仅发版或显式调用。
#
# 可自动化项（--auto）：产物存在、launch-check-tauri、verify-packaged-app。
# 人确认项（--record）：录制切换、助手出字、文件/链接摄入。
set -uo pipefail

cd "$(dirname "$0")/../.."

auto=0
record=0
launch=0
for arg in "$@"; do
  case "${arg}" in
    --auto) auto=1 ;;
    --record) record=1 ;;
    --launch) launch=1 ;;
    --help|-h)
      echo "用法：$0 [--auto] [--record] [--launch]"
      echo "  （无参）   打印发版冒烟清单与当前产物状态（不跑真机冒烟）"
      echo "  --auto     跑可自动化项：dmg 存在 → launch-check → 产物内 daemon"
      echo "  --record   逐项询问人确认结果，写进 docs/internal/packaged-smoke-<日期>.md"
      echo "  --launch   挂载 dmg 并用临时数据目录拉起应用（便于人点）"
      echo
      echo "发版路径：先 ./scripts/package-macos-tauri.sh [--with-smoke]，"
      echo "再本脚本 --auto，必要时 --launch --record。"
      echo "日常提交不要跑本清单；提交冒烟用 ./scripts/verify-affected.sh --with-smoke。"
      exit 0
      ;;
    *)
      echo "错误：未知参数 ${arg}" >&2
      echo "用法：$0 [--auto] [--record] [--launch]" >&2
      exit 2
      ;;
  esac
done

dmg="$(ls -t src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)"
app="$(ls -d src-tauri/target/release/bundle/macos/*.app 2>/dev/null | head -1 || true)"

cat <<'CHECKLIST'
== 发版前打包产物冒烟清单 ==

前置：`./scripts/package-macos-tauri.sh` 出 dmg；交付前建议加 `--with-smoke`
（产物启动验收）。本清单在打包之后跑，不替代日常 `verify-affected`。

R1 打包 / 安装产物
   判据：`src-tauri/target/release/bundle/dmg/*.dmg` 存在；包内有可执行
   `MineContext.app/Contents/Resources/backend/mc-daemon`。
   自动：`--auto` 检查 dmg，并跑 `verify-packaged-app.sh`。

R2 daemon 就绪 / 渲染层未卡住
   判据：挂载 dmg 启动后写出 `runtime.json`，渲染层完成 bootstrap，无白屏/卡在加载页；
   SIGTERM 后清理且进程退出。
   自动：`--auto` 跑 `./scripts/tests/launch-check-tauri.sh`。

R3 录制开关（人确认）
   判据：界面或托盘切换「开始 / 暂停录制」一次 → 屏幕监控页采集状态变化
   （托盘文案或监控页记录有可见变化）。

R4 助手一条消息（人确认）
   判据：助手页发送一条消息 → 有流式出字或明确错误（非空白卡死）。

R5 文件或链接摄入一条路径（人确认）
   判据：任选其一完成一次：上传/导入一个本地文件，或提交一个链接 →
   出现入库/解析中的可见反馈（成功条目或明确失败），界面不卡死。

人确认项用 `--record` 逐项登记；需要当场操作时加 `--launch`。
托盘菜单、通知、开机自启等观感见 `./scripts/tests/manual-smoke.sh`。
CHECKLIST

echo
if [ -n "${dmg}" ]; then
  echo "产物：${dmg}"
else
  echo "产物：尚未找到 dmg（先跑 ./scripts/package-macos-tauri.sh）"
fi
if [ -n "${app}" ]; then
  echo "应用包：${app}"
else
  echo "应用包：磁盘上无 .app（可能已被 Tauri 清理；--auto 仍可用 dmg + launch-check）"
fi
echo

fail=0
app_pid=""
mount_point=""
work_dir=""

cleanup_launch() {
  if [ -n "${app_pid}" ] && kill -0 "${app_pid}" 2>/dev/null; then
    kill -TERM "${app_pid}" 2>/dev/null || true
  fi
  if [ -n "${mount_point}" ]; then
    pkill -f "${mount_point}/MineContext.app/Contents/MacOS/" 2>/dev/null || true
    hdiutil detach "${mount_point}" -force -quiet 2>/dev/null || true
    rmdir "${mount_point}" 2>/dev/null || true
  fi
  if [ -n "${work_dir}" ]; then
    chmod -R u+w "${work_dir}" 2>/dev/null || true
    rm -rf "${work_dir}" 2>/dev/null || true
  fi
}

run_auto() {
  if [ -z "${dmg}" ]; then
    echo "FAIL: 没有打包产物 dmg。先跑：./scripts/package-macos-tauri.sh" >&2
    return 1
  fi

  echo "== R1/R2 可自动化项 =="
  if [ -n "${app}" ]; then
    if ! ./scripts/tests/verify-packaged-app.sh "${app}"; then
      echo "FAIL: 产物内 daemon 校验未通过" >&2
      return 1
    fi
  else
    echo "SKIP: 磁盘无 .app，跳过 verify-packaged-app（launch-check 会挂载 dmg 验启动）"
  fi

  if ! ./scripts/tests/launch-check-tauri.sh "${dmg}"; then
    echo "FAIL: 产物启动验收未通过（daemon/渲染层健康）" >&2
    return 1
  fi
  echo "PASS: R1/R2 可自动化项通过"
  echo
  echo "仍须人确认：R3 录制、R4 助手、R5 文件或链接摄入（加 --record）。"
  return 0
}

run_launch() {
  if [ -z "${dmg}" ]; then
    echo "FAIL: 没有 dmg，无法 --launch。先跑：./scripts/package-macos-tauri.sh" >&2
    return 1
  fi
  if ! command -v hdiutil >/dev/null 2>&1; then
    echo "FAIL: 当前环境没有 hdiutil，无法挂载 dmg" >&2
    return 1
  fi

  mount_point="$(mktemp -d)"
  work_dir="$(mktemp -d)"
  trap cleanup_launch EXIT

  hdiutil detach "${mount_point}" -force -quiet 2>/dev/null || true
  if ! hdiutil attach "${dmg}" -mountpoint "${mount_point}" -nobrowse -quiet; then
    echo "FAIL: dmg 挂载失败" >&2
    return 1
  fi

  local binary
  binary="$(ls "${mount_point}/MineContext.app/Contents/MacOS/" 2>/dev/null | head -1 || true)"
  if [ -z "${binary}" ]; then
    echo "FAIL: 包结构不对，找不到可执行文件" >&2
    return 1
  fi

  echo "== 拉起应用（数据目录 ${work_dir}） =="
  MC_DATA_DIR="${work_dir}" "${mount_point}/MineContext.app/Contents/MacOS/${binary}" \
    >"${work_dir}/shell.log" 2>&1 &
  app_pid=$!
  sleep 12
  if ! kill -0 "${app_pid}" 2>/dev/null; then
    echo "FAIL: 应用提前退出，日志：" >&2
    tail -20 "${work_dir}/shell.log" >&2 || true
    return 1
  fi
  echo "应用在运行（pid ${app_pid}）。daemon："
  pgrep -fl "Resources/backend/mc-daemon" || echo "  （没看到 daemon —— 记为缺陷）"
  echo
  return 0
}

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
  echo "| ${item} | ${verdict} | ${note} |" >>"${record_out}"
}

run_record() {
  mkdir -p docs/internal
  record_out="docs/internal/packaged-smoke-$(date +%Y%m%d-%H%M).md"
  {
    echo "# 打包产物冒烟记录（$(date '+%Y-%m-%d %H:%M')）"
    echo
    echo "- 产物：\`${dmg:-（无）}\`"
    echo "- 机器：$(uname -s) $(uname -m)"
    echo "- 方式：\`scripts/tests/packaged-smoke-checklist.sh --record\`"
    echo
    echo "| 项 | 结果 | 现象 / 备注 |"
    echo "|---|---|---|"
  } >"${record_out}"

  ask "R3 录制开关" "切换开始/暂停录制后，采集状态是否可见变化？"
  ask "R4 助手一条消息" "助手页发一条消息后，是否流式出字或明确错误（非卡死）？"
  ask "R5 文件或链接摄入" "文件上传或链接提交一条路径是否有可见反馈且界面不卡死？"

  {
    echo
    echo "记录人：（请填）"
    echo
    echo "同步：不通过项记入 docs/operations.md §5.1 / §7；自动项用 --auto 复验。"
  } >>"${record_out}"

  echo
  echo "结果已写入 ${record_out}"
}

if [ "${auto}" -eq 1 ]; then
  if ! run_auto; then
    fail=1
  fi
fi

if [ "${launch}" -eq 1 ]; then
  if ! run_launch; then
    fail=1
  fi
fi

if [ "${record}" -eq 1 ]; then
  run_record
fi

if [ "${auto}" -eq 0 ] && [ "${launch}" -eq 0 ] && [ "${record}" -eq 0 ]; then
  echo "（只打印清单。发版时：--auto 跑 R1/R2；人确认加 --record；操作界面加 --launch。）"
fi

if [ "${launch}" -eq 1 ] && [ -n "${app_pid}" ]; then
  if [ "${record}" -ne 1 ]; then
    echo "确认完在界面上退出即可；本脚本结束时也会收掉它。"
  fi
  wait "${app_pid}" 2>/dev/null || true
  app_pid=""
fi

exit "${fail}"
