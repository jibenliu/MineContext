#!/usr/bin/env bash
# Tauri 外壳的 macOS 打包：release daemon → Tauri 打包。
#
# 签名：无 Developer ID → **adhoc 签名、未公证**（signingIdentity "-"）。
# 不假装已公证；从网上下载后仍可能被隔离，需 xattr 清 quarantine。
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

echo "== Tauri 打包（adhoc 签名，未公证） =="
# signingIdentity "-"：adhoc 签名，减轻「已损坏」误报；仍无 Developer ID / 公证，
# 从网上下载后仍可能被隔离，需 xattr 清除 quarantine（见文末说明）。
(cd src-tauri && cargo tauri build --config '{"bundle":{"resources":{"../target/release/mc-daemon":"backend/mc-daemon"},"macOS":{"signingIdentity":"-"}}}')

app="$(ls -d src-tauri/target/release/bundle/macos/*.app 2>/dev/null | head -1 || true)"
if [ -z "${app}" ]; then
  echo "FAIL: 没有产出 .app（无法校验包内 daemon）"
  exit 1
fi
bundled_daemon="${app}/Contents/Resources/backend/mc-daemon"
if [ ! -f "${bundled_daemon}" ]; then
  echo "FAIL: 包内缺少 ${bundled_daemon}（打包资源配置未把 release daemon 打进 Resources）"
  exit 1
fi
chmod u+x "${bundled_daemon}" 2>/dev/null || true
if [ ! -x "${bundled_daemon}" ]; then
  echo "FAIL: 包内 daemon 不可执行：${bundled_daemon}"
  exit 1
fi
echo "PASS: 包内有可执行 daemon（${bundled_daemon}）"

if command -v codesign >/dev/null 2>&1; then
  echo "== 确认 adhoc 签名：${app} =="
  codesign --force --deep --sign - "${app}"
  codesign --verify --verbose=2 "${app}" || true
fi

dmg="$(ls -t src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)"
if [ -z "${dmg}" ]; then
  echo "FAIL: 没有产出 release dmg"
  exit 1
fi
# 若 .app 在 dmg 生成后才补签，重建 dmg，避免盘里仍是未签名包
if [ -n "${app}" ] && command -v hdiutil >/dev/null 2>&1; then
  version="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
  arch="$(uname -m)"
  case "$arch" in
    arm64 | aarch64) arch="aarch64" ;;
    x86_64 | amd64) arch="x86_64" ;;
  esac
  rebuilt="src-tauri/target/release/bundle/dmg/MineContext_${version}_${arch}.dmg"
  stage="$(mktemp -d)"
  cp -R "${app}" "${stage}/"
  rm -f "${rebuilt}"
  hdiutil create -volname "MineContext" -srcfolder "${stage}" -ov -format UDZO "${rebuilt}" >/dev/null
  rm -rf "${stage}"
  dmg="${rebuilt}"
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
echo "adhoc 签名、未公证（无 Developer ID）。若提示「已损坏」："
echo "  xattr -cr ~/Downloads/MineContext_*.dmg"
echo "  xattr -dr com.apple.quarantine /Applications/MineContext.app"
echo "或对 .app 右键 →「打开」。"
