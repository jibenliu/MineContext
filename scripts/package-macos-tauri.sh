#!/usr/bin/env bash
# Tauri 外壳的 macOS 打包：release daemon → Tauri 打包。
#
# 签名：本仓库没有 Developer ID，产物**未签名未公证**，如实登记（不假装已签名）。
# dmg 的 resources 用 `--config` 覆盖成 **release** 版 daemon：仓库里
# `tauri.conf.json` 默认指向 debug（开发态方便），release 包必须带 release 二进制。
set -euo pipefail

cd "$(dirname "$0")/.."

with_smoke=0
for argument in "$@"; do
  case "$argument" in
    --with-smoke) with_smoke=1 ;;
    --help|-h)
      echo '用法：./scripts/package-macos-tauri.sh [--with-smoke]'
      echo '默认仅构建；--with-smoke 显式运行产物启动验收，失败返回非零。'
      exit 0 ;;
    *) echo "错误：未知参数 $argument" >&2; exit 2 ;;
  esac
done

echo "== 构建 release daemon =="
cargo build --release -p mc-daemon
# tauri build-script 会先校验 tauri.conf.json 里的默认 resources（debug 路径）；
# 部分 CLI 上 `--config` 覆盖晚于该校验，CI 上会报 debug/mc-daemon 不存在。
# 把 release 二进制同步到该路径，打包时再用 --config 指到 release。
mkdir -p target/debug
cp -f target/release/mc-daemon target/debug/mc-daemon

echo "== 渲染层产物 =="
(cd frontend && npx --no-install vite build >/dev/null && echo "vite build: ok")

echo "== Tauri 打包（未签名） =="
(cd src-tauri && cargo tauri build --config '{"bundle":{"resources":{"../target/release/mc-daemon":"backend/mc-daemon"}}}')

dmg="$(ls -t src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)"
if [ -z "${dmg}" ]; then
  echo "FAIL: 没有产出 release dmg"
  exit 1
fi
echo "产物：${dmg}（$(du -h "${dmg}" | cut -f1)）"
shasum -a 256 "${dmg}"

if [ "$with_smoke" -eq 1 ]; then
  # 交付路径只出一行结论：逐条 PASS 是**测试**日志，打包输出里不需要。
  # 失败时把完整诊断（含保留的 work_dir）原样打出来 —— 那时它才是有用的信息。
  echo "== 产物启动验收（显式启用） =="
  if ! check_output="$(./scripts/tests/launch-check-tauri.sh "${dmg}" 2>&1)"; then
    printf '%s\n' "${check_output}"
    echo "错误：产物已生成，但启动验收失败，不可作为验收通过的版本交付：${dmg}" >&2
    exit 1
  fi
  echo "产物启动验收通过（dmg → 挂载 → 启动 → 渲染层 bootstrap → daemon 就绪 → 退出清理）"
  echo "打包完成：${dmg}"
else
  echo "构建完成（未运行启动验收）：${dmg}"
  echo '交付前验收：./scripts/package-macos-tauri.sh --with-smoke'
fi
echo "未签名（无 Developer ID）：首次打开需要右键「打开」，或"
echo "  xattr -dr com.apple.quarantine /Applications/MineContext.app"
