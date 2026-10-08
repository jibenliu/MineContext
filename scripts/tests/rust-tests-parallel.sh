#!/usr/bin/env bash
# 并行跑 Rust 测试：先把所有测试二进制构建一次，再按并发度分别执行。
#
# 为什么不是 `cargo test --workspace`：那条命令把 129 个测试二进制**串行**跑完，
# 实测 40 分钟；其中 85% 的时间花在等少数几个慢二进制（capture_api / e2e / cli），
# 而这台机器有 12 个核。改成并发执行后实测 11 分钟，且结果完全一致（1155 条全绿）。
#
# 环境变量：
#   MC_TEST_JOBS       并发跑几个测试二进制（默认 6；8 路并发会让起 daemon 的 e2e 抢不到 CPU）
#   MC_TEST_THREADS    每个二进制内部的线程数（默认 4）
#   MC_TEST_PKGS       只跑这些包（`-p` 过滤，空格分隔；默认整个工作区）
set -uo pipefail

cd "$(dirname "$0")/../.."

PAR="${MC_TEST_JOBS:-6}"
THREADS="${MC_TEST_THREADS:-4}"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

# 时限：挂住的测试必须变成**可见的失败**，而不是把门禁卡到天荒地老
timeout_cmd() {
  if command -v timeout >/dev/null 2>&1; then
    timeout "$@"
  elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout "$@"
  else
    "$@"
  fi
}

pkg_args=(--workspace)
if [ -n "${MC_TEST_PKGS:-}" ]; then
  pkg_args=()
  for pkg in ${MC_TEST_PKGS}; do pkg_args+=(-p "${pkg}"); done
fi

echo "构建测试二进制…"
if ! timeout_cmd 2700 cargo test "${pkg_args[@]}" --no-run --message-format=json \
  >"${WORK}/artifacts.json" 2>"${WORK}/build.log"; then
  echo "测试二进制构建失败，最后 40 行："
  tail -40 "${WORK}/build.log"
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "错误：并行跑测试需要 jq 解析 cargo 的 JSON 输出（macOS 上 brew install jq）。"
  exit 1
fi

# name<TAB>可执行文件<TAB>crate 根目录（测试以 crate 根为工作目录）
jq -r 'select(.reason == "compiler-artifact" and .executable != null and .profile.test == true)
       | [.target.name, .executable, (.manifest_path | rtrimstr("/Cargo.toml"))] | @tsv' \
  "${WORK}/artifacts.json" | sort -u >"${WORK}/bins.tsv"
total="$(wc -l <"${WORK}/bins.tsv" | tr -d ' ')"
echo "共 ${total} 个测试二进制，并发 ${PAR}（每个内部 ${THREADS} 线程）"

run_one() {
  local key="$1" name="$2" exe="$3" cwd="$4"
  local t0=${SECONDS}
  (cd "${cwd}" && timeout_cmd 900 "${exe}" --test-threads "${THREADS}") \
    >"${WORK}/${key}.log" 2>&1
  printf '%s\t%s\t%s\n' "$?" "$((SECONDS - t0))" "${name}" >"${WORK}/${key}.rc"
}

start=${SECONDS}
i=0
while IFS="$(printf '\t')" read -r name exe cwd; do
  [ -z "${name}" ] && continue
  i=$((i + 1))
  key="$(printf '%03d' "${i}")"
  while [ "$(jobs -rp | wc -l | tr -d ' ')" -ge "${PAR}" ]; do sleep 0.2; done
  run_one "${key}" "${name}" "${exe}" "${cwd}" &
done <"${WORK}/bins.tsv"
wait
elapsed=$((SECONDS - start))

pass_bins="$(grep -l -E '^test result: ok' "${WORK}"/*.log 2>/dev/null | wc -l | tr -d ' ')"
failed_bins="$(cat "${WORK}"/*.rc | awk -F'\t' '$1 != 0' | wc -l | tr -d ' ')"
passed="$(grep -h -E '^test result: ok' "${WORK}"/*.log 2>/dev/null \
  | awk -F'[ .;]+' '{s += $4} END {print s + 0}')"

echo
echo "用时 $((elapsed / 60)) 分 $((elapsed % 60)) 秒；二进制 ${pass_bins}/${total} 全绿；断言 ${passed} 条"

if [ "${failed_bins}" -ne 0 ]; then
  echo
  echo "== 失败的测试二进制 =="
  cat "${WORK}"/*.rc | awk -F'\t' '$1 != 0 {printf "  %s（退出码 %s，用时 %ss）\n", $3, $1, $2}'
  for f in "${WORK}"/*.rc; do
    key="$(basename "$f" .rc)"
    [ "$(cut -f1 "$f")" = "0" ] && continue
    echo
    echo "---- ${key} 最后 30 行 ----"
    tail -30 "${WORK}/${key}.log"
  done
  echo
  echo "完整日志留在 ${WORK}（本次结束后会被清理，需要时重跑单个 crate：cargo test -p <crate>）"
  # 保留失败现场：把日志复制到 target/ 下，方便事后查看
  mkdir -p target/rust-test-logs
  cp "${WORK}"/*.log "${WORK}"/*.rc target/rust-test-logs/ 2>/dev/null || true
  echo "失败日志已复制到 target/rust-test-logs/"
  exit 1
fi

echo "Rust tests passed: ${passed}"
