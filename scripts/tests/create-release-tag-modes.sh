#!/usr/bin/env bash
# create-release-tag.sh：dry-run / 版本不一致 / 非法版本 / --assert-only 必须可拦截。
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT

mkdir -p "$fixture/frontend" "$fixture/src-tauri" "$fixture/scripts/lib" "$fixture/.git"
cp "$root/scripts/create-release-tag.sh" "$fixture/scripts/"
# 闸门逻辑在 lib；fixture 必须带上，否则脚本 source 失败
cp "$root/scripts/lib/version-gate.sh" "$fixture/scripts/lib/"
chmod +x "$fixture/scripts/create-release-tag.sh"

# 最小 git 仓库：脚本要读 HEAD
git -C "$fixture" init -q
git -C "$fixture" config user.email "test@example.com"
git -C "$fixture" config user.name "test"
printf '[workspace.package]\nversion = "0.2.0"\n' > "$fixture/Cargo.toml"
printf '[package]\nname = "mc-desktop"\nversion = "0.2.0"\n' > "$fixture/src-tauri/Cargo.toml"
printf '{\n  "name": "MineContext",\n  "version": "0.2.0"\n}\n' > "$fixture/frontend/package.json"
printf '{\n  "version": "0.2.0"\n}\n' > "$fixture/src-tauri/tauri.conf.json"
git -C "$fixture" add .
git -C "$fixture" commit -qm "fixture"

# stub 远端探测：无 origin 时 ls-remote 会失败，脚本应仍能 dry-run
cd "$fixture"

out="$(./scripts/create-release-tag.sh --dry-run)"
echo "$out" | grep -q 'v0.2.0'
echo "$out" | grep -q 'dry-run'

if ./scripts/create-release-tag.sh --dry-run 9.9.9 >/tmp/tag-mismatch.log 2>&1; then
  echo 'FAIL: 入参与仓库版本不一致却通过'
  exit 1
fi
grep -q '不一致' /tmp/tag-mismatch.log

if ./scripts/create-release-tag.sh --dry-run not-a-version >/tmp/tag-bad.log 2>&1; then
  echo 'FAIL: 非法版本却通过'
  exit 1
fi
grep -q '非法版本号' /tmp/tag-bad.log

# 四处漂移：Cargo 仍是 0.2.0，前端落后到 0.1.6（v1.0.7 打 tag 时的真实事故形态）
printf '{\n  "name": "MineContext",\n  "version": "0.1.6"\n}\n' > "$fixture/frontend/package.json"
if ./scripts/create-release-tag.sh --assert-only >/tmp/tag-drift.log 2>&1; then
  echo 'FAIL: 四处版本漂移却通过'
  exit 1
fi
grep -qE '不一致|漂移' /tmp/tag-drift.log
# 恢复一致
printf '{\n  "name": "MineContext",\n  "version": "0.2.0"\n}\n' > "$fixture/frontend/package.json"

# --assert-only：只校验、不建 tag；期望版本与仓库不符必须失败（Release CI 闸门）
if ./scripts/create-release-tag.sh --assert-only v0.1.6 >/tmp/tag-assert-mismatch.log 2>&1; then
  echo 'FAIL: --assert-only 期望版本与仓库不符却通过'
  exit 1
fi
grep -q '不一致' /tmp/tag-assert-mismatch.log

out="$(./scripts/create-release-tag.sh --assert-only v0.2.0)"
echo "$out" | grep -q '版本闸门通过'
# 不得创建标签
if git -C "$fixture" rev-parse -q --verify 'refs/tags/v0.2.0' >/dev/null 2>&1; then
  echo 'FAIL: --assert-only 不应创建标签'
  exit 1
fi

echo 'PASS: create-release-tag modes'
