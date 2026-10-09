#!/usr/bin/env bash
# 改动面定向验证：按「改了什么」选最小检查集，秒级到分钟级给反馈。
#
#   ./scripts/verify-affected.sh            # 与 HEAD 比较（默认看未提交改动）
#   ./scripts/verify-affected.sh <ref>      # 与任意 ref 比较，例如 origin/main
#   ./scripts/verify-affected.sh --list     # 只打印将要跑什么，不执行
#   ./scripts/verify-affected.sh --with-smoke # 提交前显式加入冒烟
#
# 全量门禁（scripts/verify-all.sh）留给阶段/里程碑收尾与跨切面改动；
# 日常改一两行只跑这里。结尾会列出**没跑**的部分——定向验证不等于全量通过。
set -uo pipefail

cd "$(dirname "$0")/.."

. "$(dirname "$0")/./lib/xtask.sh"

REF="HEAD"
LIST_ONLY=0
RUN_SMOKE=0
for arg in "$@"; do
  case "$arg" in
    --list) LIST_ONLY=1 ;;
    --with-smoke) RUN_SMOKE=1 ;;
    -h | --help)
      sed -n '2,10p' "$0"
      exit 0
      ;;
    *) REF="$arg" ;;
  esac
done

changed="$( (git diff --name-only "$REF"; git ls-files -o --exclude-standard) 2>/dev/null | sort -u | grep -v '^$' || true)"
if [ -z "$changed" ] && [ "$RUN_SMOKE" -eq 0 ]; then
  echo "与 ${REF} 相比没有改动，无需验证。"
  exit 0
fi

# ---- 分桶 ---------------------------------------------------------------
rust_pkgs=""      # 受影响的 crate（含一层反向依赖），包名列表
fe_lint=0
fe_typecheck=0
fe_adapters=0
fe_shared=0
fe_pages=0
fe_build=0
do_docs=0
do_scripts=0
do_contract=0
do_tauri=0
do_workspace=0

pkg_of() { sed -n 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$1/Cargo.toml" | head -1; }
add_pkg() { case " ${rust_pkgs} " in *" $1 "*) ;; *) rust_pkgs="${rust_pkgs} $1" ;; esac; }

add_path() {
  local p="$1"
  case "$p" in
    crates/*/* | apps/*/*)
      local dir="${p%%/*}/$(echo "$p" | cut -d/ -f2)"
      [ -f "${dir}/Cargo.toml" ] && add_pkg "$(pkg_of "$dir")"
      ;;
    Cargo.toml | Cargo.lock | rust-toolchain* | .github/workflows/*) do_workspace=1 ;;
    frontend/*)
      fe_lint=1
      case "$p" in
        *.ts | *.tsx | *.mts | *.cts) fe_typecheck=1 ;;
      esac
      case "$p" in
        frontend/src/renderer/src/adapters/*) fe_adapters=1 ;;
        frontend/packages/*) fe_shared=1 ;;
      esac
      case "$p" in
        frontend/src/renderer/* | frontend/vite.config.mts | frontend/index.html) fe_build=1 ;;
      esac
      case "$p" in
        frontend/src/renderer/*) fe_pages=1 ;;
        frontend/*.json | frontend/*.mts | frontend/*.ts | frontend/*.mjs) fe_build=1 ;;
      esac
      ;;
    fixtures/contract/*) do_contract=1 ;;
    src-tauri/*) do_tauri=1 ;;
    scripts/*) do_scripts=1 ;;
    docs/* | *.md) do_docs=1 ;;
  esac
}

for p in $changed; do add_path "$p"; done

# 一层反向依赖：改 mc-storage 也要跑 mc-server 的测试（它的调用方）
meta="$(mktemp)"
trap 'rm -f "$meta" "$log"' EXIT
log="$(mktemp)"
if [ -n "${rust_pkgs}" ] && command -v jq >/dev/null 2>&1; then
  cargo metadata --no-deps --format-version 1 >"$meta" 2>/dev/null || true
  for pkg in ${rust_pkgs}; do
    for dep in $(jq -r --arg p "${pkg}" \
      '.packages[] | select(any(.dependencies[]?; .name == $p)) | .name' "$meta" 2>/dev/null); do
      add_pkg "$dep"
    done
  done
fi
[ "${do_workspace}" -eq 1 ] && rust_pkgs=""

# ---- 执行 ---------------------------------------------------------------
fail=0
timed() {
  local name="$1"
  shift
  local t0=${SECONDS}
  if "$@" >"$log" 2>&1; then
    printf '  ✅ %-46s %2ds\n' "${name}" "$((SECONDS - t0))"
  else
    printf '  ❌ %-46s %2ds\n' "${name}" "$((SECONDS - t0))"
    sed 's/^/       /' "$log" | tail -25
    fail=1
  fi
}
in_fe() { (cd frontend && "$@"); }
check() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

[ "${LIST_ONLY}" -eq 1 ] && echo "改动文件：$(echo "$changed" | wc -l | tr -d ' ') 个"

check "Rust（${rust_pkgs:-无}）"
if [ "${do_workspace}" -eq 1 ]; then
  echo "  工作区清单/CI 有改动 → 跑全量 Rust 测试（慢路径，值得）"
  [ "${LIST_ONLY}" -eq 0 ] && timed "全量 Rust 测试（并行）" ./scripts/tests/rust-tests-parallel.sh
elif [ -n "${rust_pkgs}" ]; then
  [ "${LIST_ONLY}" -eq 0 ] && timed "cargo fmt --check" cargo fmt --all -- --check
  for pkg in ${rust_pkgs}; do
    [ "${LIST_ONLY}" -eq 0 ] && timed "clippy -p ${pkg}" cargo clippy -q -p "${pkg}" --all-targets -- -D warnings
  done
  # 测试也并行：受影响包可能是 mc-server 这种串行要 4 分钟的
  [ "${LIST_ONLY}" -eq 0 ] && MC_TEST_PKGS="${rust_pkgs}" timed "受影响包测试（并行）" \
    ./scripts/tests/rust-tests-parallel.sh
else
  echo "  没有 Rust 改动"
fi

if [ "${do_contract}" -eq 1 ]; then
  check "契约新鲜度"
  [ "${LIST_ONLY}" -eq 0 ] && timed "extract-used-ipc-channels + git diff" \
    bash -c '. ./scripts/lib/xtask.sh && xtask_run extract-used-ipc-channels && git diff --quiet fixtures/contract/'
fi

if [ "${do_scripts}" -eq 1 ]; then
  check "脚本与守卫"
  [ "${LIST_ONLY}" -eq 0 ] && timed "shell 变量展开" ./scripts/checks/check-shell-vars.sh
  case " ${changed} " in
    *scripts/tests/rust-tests-parallel.sh* | *scripts/checks/check-timeout-cmd.sh*)
      [ "${LIST_ONLY}" -eq 0 ] && timed "timeout_cmd 回退（macOS）" ./scripts/checks/check-timeout-cmd.sh
      ;;
  esac
  case " ${changed} " in
    *scripts/create-release-tag.sh* | *scripts/tests/create-release-tag-modes.sh* | *tag-release.yml*)
      [ "${LIST_ONLY}" -eq 0 ] && timed "发版标签脚本模式" ./scripts/tests/create-release-tag-modes.sh
      ;;
  esac
  [ "${LIST_ONLY}" -eq 0 ] && timed "源码类门禁（10 条）" ./scripts/check-source.sh
  case " ${changed} " in
    *scripts/tests/selftest-lints.sh* | *scripts/checks/*)
      [ "${LIST_ONLY}" -eq 0 ] && timed "守卫自检（植入违规必须被拦）" ./scripts/tests/selftest-lints.sh
      ;;
  esac
fi

if [ "${do_docs}" -eq 1 ]; then
  check "文档"
  [ "${LIST_ONLY}" -eq 0 ] && timed "故障排查文档与错误码一致" ./scripts/check-docs.sh
fi

if [ "${do_tauri}" -eq 1 ]; then
  check "Tauri 外壳"
  [ "${LIST_ONLY}" -eq 0 ] && timed "src-tauri cargo check" bash -c 'cd src-tauri && cargo check --quiet'
fi

if [ "${fe_lint}" -eq 1 ]; then
  check "前端"
  [ "${LIST_ONLY}" -eq 0 ] && timed "样式变量（可解析 + 跟随主题）" ./scripts/checks/check-theme-tokens.sh
  [ "${LIST_ONLY}" -eq 0 ] && timed "eslint" in_fe npm run --silent lint
  [ "${fe_typecheck}" -eq 1 ] && [ "${LIST_ONLY}" -eq 0 ] && timed "tsc" in_fe npm run --silent typecheck
  [ "${fe_adapters}" -eq 1 ] && [ "${LIST_ONLY}" -eq 0 ] && timed "适配层测试" in_fe npm run --silent test:adapters
  [ "${fe_shared}" -eq 1 ] && [ "${LIST_ONLY}" -eq 0 ] && timed "共享包测试" in_fe npm run --silent test:shared
  [ "${fe_pages}" -eq 1 ] && [ "${LIST_ONLY}" -eq 0 ] && timed "页面测试" in_fe npx --no-install vitest run --config vitest.config.ts
  [ "${fe_build}" -eq 1 ] && [ "${LIST_ONLY}" -eq 0 ] && timed "生产构建" in_fe npx --no-install vite build
fi

if [ "$RUN_SMOKE" -eq 1 ]; then
  check "提交/交付冒烟（显式请求）"
  [ "$LIST_ONLY" -eq 0 ] && timed "真进程与产物冒烟" ./scripts/verify-smoke.sh
fi

check "汇总"
if [ "${fail}" -ne 0 ]; then
  echo "定向验证未通过。"
  exit 1
fi
if [ "${LIST_ONLY}" -eq 1 ]; then
  echo "以上是将要执行的检查（--list 模式没有真的跑）。"
  exit 0
fi
echo "定向验证通过（本次只覆盖改动面）。"
if [ "$RUN_SMOKE" -eq 0 ]; then
  echo "未跑冒烟：提交前用 --with-smoke，打版本/最终交付前用 ./scripts/verify-all.sh --with-smoke。"
fi
