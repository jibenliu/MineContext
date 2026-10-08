#!/usr/bin/env bash
# 一条命令跑完全部门禁。
#
# 与 CI (.github/workflows/rust.yml) 保持一致，便于本地复现 CI 结果。
set -euo pipefail

MC_RUN_SMOKE=0
for arg in "$@"; do
  case "$arg" in
    --with-smoke) MC_RUN_SMOKE=1 ;;
    -h | --help)
      echo "用法：$0 [--with-smoke]"
      echo "默认只验证源码与业务测试；提交/打版本前显式加入冒烟。"
      exit 0 ;;
    *) echo "未知参数：$arg" >&2; exit 2 ;;
  esac
done
export MC_RUN_SMOKE

cd "$(dirname "$0")/.."

. "$(dirname "$0")/./lib/xtask.sh"

# 每步计时：门禁"慢"的时候，先看清时间花在哪一步，而不是猜。
# （全量动辄几十分钟，不逐步计时就无法判断时间花在哪一步。）
STEP_START=${SECONDS}
STEP_NAME=""
GATE_START=${SECONDS}

step() {
  if [ -n "${STEP_NAME}" ]; then
    printf '  ⏱ %s 耗时 %d 分 %d 秒\n' "${STEP_NAME}" \
      $(( (SECONDS - STEP_START) / 60 )) $(( (SECONDS - STEP_START) % 60 ))
  fi
  STEP_NAME="$1"
  STEP_START=${SECONDS}
  printf '\n\033[1m== %s ==\033[0m\n' "$1"
}

step "1/8 契约新鲜度（渠道表 + 调用点反查）"
# 兼容面路由清单已冻结（旧后端源码不再存在），由 mc-server 的
# `compat_routes_all_present` 测试保证「清单里的路径一条都不少」。
xtask_run extract-used-ipc-channels
if ! git diff --quiet fixtures/contract/; then
  echo "错误：fixtures/contract/ 与源码不一致，请提交工具产物"
  git --no-pager diff --stat fixtures/contract/
  exit 1
fi

step "2/8 lint 脚本自检（植入违规必须被拦截）"
./scripts/tests/selftest-lints.sh

# 四类门禁各自一条入口（内部逐条打印 PASS/FAIL，红了能看出是哪条）
./scripts/check-source.sh
./scripts/check-macos.sh
./scripts/check-docs.sh
./scripts/check-tests.sh

# shell 脚本的变量展开（bash 3.2 会把 `$var（` 里的全角标点当成变量名）
./scripts/checks/check-shell-vars.sh

step "3/8 格式"
cargo fmt --all -- --check
echo "ok"

step "4/8 Clippy（警告即错误）"
cargo clippy --workspace --all-targets -- -D warnings

step "5/8 Rust 测试"
# 并行跑全部测试二进制：串行 `cargo test --workspace` 实测 40 分钟，
# 而 12 核的机器只用到 1 个核。并行版实测 11 分钟、结果一致（见该脚本头部）。
# 需要串行排查时：MC_SERIAL_RUST_TESTS=1 ./scripts/verify-all.sh
if [ "${MC_SERIAL_RUST_TESTS:-0}" = "1" ]; then
  echo "MC_SERIAL_RUST_TESTS=1：走串行 cargo test --workspace"
  cargo test --workspace
else
  ./scripts/tests/rust-tests-parallel.sh
fi

step "6/8 可选交付冒烟"
if [ "$MC_RUN_SMOKE" -eq 1 ]; then
  ./scripts/verify-smoke.sh
else
  echo "SKIP: 冒烟与产物启动检查；提交/打版本前运行 --with-smoke。"
fi

step "7/8 前端（lint + 类型 + 三层测试 + 生产构建）"
# 五件事，缺一件都会留下「本地绿、打包红」的口子：
# 0. lint（eslint，配置在 eslint.config.mjs；只有 warning 时不失败）；
# 1. 类型检查（tsc，配置自身 + 渲染层两份 tsconfig）；
# 2. 适配层测试（`node --test`，不需要依赖）：渠道映射与后端切换对不对；
# 3. 共享包测试（`node --test`）：日志前缀与落盘出口；
# 4. 页面级测试（vitest + jsdom）：组件**真的渲染出**后端的数据；
# 5. 生产构建（纯 vite，外壳是 Tauri）：产物能打出来 —— 组件测试说明不了这件事。
(cd frontend && npm run --silent lint)
(cd frontend && npm run --silent typecheck)
(cd frontend && npm run --silent test:adapters 2>&1 | grep -E '^ℹ (tests|pass|fail)')
# 共享包（渲染层日志器）：前缀、落盘出口的时机与容错
(cd frontend && npm run --silent test:shared 2>&1 | grep -E '^ℹ (tests|pass|fail)')
# 完整输出落日志再取汇总行：直接 `| grep` 会把失败原因一起丢掉 ——
# 机器负载高时 vitest 可能丢掉 worker、只跑一部分文件并以非 0 退出
# （实测出现过 24/26 文件、79/84 条），只看汇总行无法判断是哪种。
page_log="/tmp/verify-all-pages.$$.log"
# `--maxWorkers=2`：vitest 的 worker 启动超时是**硬编码的 60 秒**（`START_TIMEOUT = 6e4`，
# 配置项改不了）。默认按 CPU 数开一堆 forks，机器一慢就会出现「启动 worker 就超时」，
# 于是页面层只跑完一部分文件、整步判红 —— 那对「测试真的挂了」是误报。
# 少开几个 worker，让每个都来得及启动；本地开发仍用默认值（`npm run test:pages`）。
if ! (cd frontend && npx --no-install vitest run --config vitest.config.ts --maxWorkers=2 >"$page_log" 2>&1); then
  echo "  页面级测试未通过，完整输出（末 30 行）："
  tail -30 "$page_log" | sed 's/^/     /'
  rm -f "$page_log"
  exit 1
fi
grep -E '^ *(Test Files|Tests) ' "$page_log"
rm -f "$page_log"
(cd frontend && npx --no-install vite build >/dev/null && echo "vite build: ok")

step "8/8 汇总"
printf '  ⏱ 门禁总耗时 %d 分 %d 秒\n' \
  $(( (SECONDS - GATE_START) / 60 )) $(( (SECONDS - GATE_START) % 60 ))
if [ "$MC_RUN_SMOKE" -eq 1 ]; then
  echo "源码与交付验证完成；产物或外部条件的 SKIP 仍需按输出补验。"
else
  echo "源码与业务测试通过；未运行冒烟，不代表交付验收通过。"
fi
