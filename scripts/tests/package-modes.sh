#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
# 避免宿主机已 export 的 Apple 凭据污染「跳过公证」夹具
unset APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
  APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID \
  APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH APPLE_API_KEY_P8 \
  API_PRIVATE_KEYS_DIR
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts/tests" "$fixture/frontend" \
  "$fixture/src-tauri/target/release/bundle/dmg" \
  "$fixture/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend" \
  "$fixture/bin"
cp "$root/scripts/package-macos-tauri.sh" "$fixture/scripts/"
touch "$fixture/src-tauri/target/release/bundle/dmg/test.dmg"
# 包内 daemon：打包脚本在出 dmg 前会校验可执行位
: > "$fixture/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend/mc-daemon"
chmod +x "$fixture/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend/mc-daemon"
cat > "$fixture/bin/cargo" <<'SH'
#!/usr/bin/env bash
echo build >> "$MC_TEST_BUILD_LOG"
# 模拟 release daemon 落盘（package-macos-tauri 会再 cp 到 target/debug/）
if [ "${MC_TEST_BUILD_EXIT:-0}" -eq 0 ]; then
  mkdir -p target/release
  : > target/release/mc-daemon
fi
exit "${MC_TEST_BUILD_EXIT:-0}"
SH
printf '#!/usr/bin/env bash\nexit 0\n' > "$fixture/bin/npx"
cat > "$fixture/scripts/tests/launch-check-tauri.sh" <<'SH'
#!/usr/bin/env bash
echo smoke >> "$MC_TEST_SMOKE_LOG"
exit "${MC_TEST_SMOKE_EXIT:-0}"
SH
chmod +x "$fixture/bin/"* "$fixture/scripts/tests/launch-check-tauri.sh"
export PATH="$fixture/bin:$PATH"
export MC_TEST_BUILD_LOG="$fixture/build.log" MC_TEST_SMOKE_LOG="$fixture/smoke.log"
# 夹具无 lib/：从仓库挂载检测库（打包脚本 source 相对 scripts/lib）
mkdir -p "$fixture/scripts/lib"
cp "$root/scripts/lib/macos-notarize-env.sh" "$fixture/scripts/lib/"
bash "$fixture/scripts/package-macos-tauri.sh" > "$fixture/default.log" 2>&1
test ! -s "$MC_TEST_SMOKE_LOG"
grep -q '未运行启动验收' "$fixture/default.log"
grep -q 'WARN: 跳过公证' "$fixture/default.log"
bash "$fixture/scripts/package-macos-tauri.sh" --with-smoke > "$fixture/explicit.log" 2>&1
test -s "$MC_TEST_SMOKE_LOG"
if MC_TEST_SMOKE_EXIT=7 bash "$fixture/scripts/package-macos-tauri.sh" --with-smoke > "$fixture/failure.log" 2>&1; then
  echo 'FAIL: 启动验收失败却返回成功'; exit 1
fi
grep -q '启动验收失败' "$fixture/failure.log"
if grep -q '打包完成' "$fixture/failure.log"; then exit 1; fi
: > "$MC_TEST_SMOKE_LOG"
if MC_TEST_BUILD_EXIT=9 bash "$fixture/scripts/package-macos-tauri.sh" --with-smoke > /dev/null 2>&1; then exit 1; fi
test ! -s "$MC_TEST_SMOKE_LOG"
: > "$MC_TEST_BUILD_LOG"
if bash "$fixture/scripts/package-macos-tauri.sh" --unknown > /dev/null 2>&1; then exit 1; fi
bash "$fixture/scripts/package-macos-tauri.sh" --help > /dev/null
test ! -s "$MC_TEST_BUILD_LOG"

# Tauri 打完 dmg 后会 Cleaning .app：只剩 dmg 时须挂载校验，不能直接 FAIL
fixture_dmg_only="$(mktemp -d)"
trap 'rm -rf "$fixture" "$fixture_dmg_only"' EXIT
mkdir -p "$fixture_dmg_only/scripts/tests" "$fixture_dmg_only/frontend" \
  "$fixture_dmg_only/scripts/lib" \
  "$fixture_dmg_only/src-tauri/target/release/bundle/dmg" \
  "$fixture_dmg_only/bin"
cp "$root/scripts/package-macos-tauri.sh" "$fixture_dmg_only/scripts/"
cp "$root/scripts/lib/macos-notarize-env.sh" "$fixture_dmg_only/scripts/lib/"
touch "$fixture_dmg_only/src-tauri/target/release/bundle/dmg/test.dmg"
cp "$fixture/bin/cargo" "$fixture_dmg_only/bin/cargo"
cp "$fixture/bin/npx" "$fixture_dmg_only/bin/npx"
cp "$fixture/scripts/tests/launch-check-tauri.sh" "$fixture_dmg_only/scripts/tests/"
cat > "$fixture_dmg_only/bin/hdiutil" <<'SH'
#!/usr/bin/env bash
if [ "$1" = attach ]; then
  mount="$4"
  mkdir -p "$mount/MineContext.app/Contents/Resources/backend"
  : > "$mount/MineContext.app/Contents/Resources/backend/mc-daemon"
  chmod +x "$mount/MineContext.app/Contents/Resources/backend/mc-daemon"
  exit 0
fi
exit 0
SH
chmod +x "$fixture_dmg_only/bin/"*
export PATH="$fixture_dmg_only/bin:$PATH"
export MC_TEST_BUILD_LOG="$fixture_dmg_only/build.log" MC_TEST_SMOKE_LOG="$fixture_dmg_only/smoke.log"
: > "$MC_TEST_BUILD_LOG"
: > "$MC_TEST_SMOKE_LOG"
bash "$fixture_dmg_only/scripts/package-macos-tauri.sh" > "$fixture_dmg_only/dmg-only.log" 2>&1
grep -q 'PASS: 包内有可执行 daemon' "$fixture_dmg_only/dmg-only.log"
grep -q '未运行启动验收' "$fixture_dmg_only/dmg-only.log"
if grep -q '没有产出 \.app' "$fixture_dmg_only/dmg-only.log"; then
  echo 'FAIL: dmg 已产出却因 .app 被清理而失败'; exit 1
fi

# 凭据齐全时不得再钉 signingIdentity "-"
fixture_notary="$(mktemp -d)"
trap 'rm -rf "$fixture" "$fixture_dmg_only" "$fixture_notary"' EXIT
mkdir -p "$fixture_notary/scripts/tests" "$fixture_notary/scripts/lib" "$fixture_notary/frontend" \
  "$fixture_notary/src-tauri/target/release/bundle/dmg" \
  "$fixture_notary/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend" \
  "$fixture_notary/bin"
cp "$root/scripts/package-macos-tauri.sh" "$fixture_notary/scripts/"
cp "$root/scripts/lib/macos-notarize-env.sh" "$fixture_notary/scripts/lib/"
touch "$fixture_notary/src-tauri/target/release/bundle/dmg/test.dmg"
: > "$fixture_notary/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend/mc-daemon"
chmod +x "$fixture_notary/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend/mc-daemon"
cat > "$fixture_notary/bin/cargo" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MC_TEST_CARGO_ARGS"
echo build >> "$MC_TEST_BUILD_LOG"
mkdir -p target/release
: > target/release/mc-daemon
exit 0
SH
printf '#!/usr/bin/env bash\nexit 0\n' > "$fixture_notary/bin/npx"
cp "$fixture/scripts/tests/launch-check-tauri.sh" "$fixture_notary/scripts/tests/"
chmod +x "$fixture_notary/bin/"* "$fixture_notary/scripts/tests/launch-check-tauri.sh"
export PATH="$fixture_notary/bin:$PATH"
export MC_TEST_BUILD_LOG="$fixture_notary/build.log"
export MC_TEST_SMOKE_LOG="$fixture_notary/smoke.log"
export MC_TEST_CARGO_ARGS="$fixture_notary/cargo.args"
: > "$MC_TEST_BUILD_LOG"
: > "$MC_TEST_SMOKE_LOG"
: > "$MC_TEST_CARGO_ARGS"
export APPLE_CERTIFICATE='Y2VydA=='
export APPLE_CERTIFICATE_PASSWORD='pw'
export APPLE_SIGNING_IDENTITY='Developer ID Application: Example (TEAMID)'
export APPLE_ID='dev@example.com'
export APPLE_PASSWORD='app-specific'
export APPLE_TEAM_ID='TEAMID1234'
bash "$fixture_notary/scripts/package-macos-tauri.sh" > "$fixture_notary/notary.log" 2>&1
grep -q 'Developer ID + 公证' "$fixture_notary/notary.log"
if grep -q 'signingIdentity":"-"' "$MC_TEST_CARGO_ARGS" || grep -q 'signingIdentity\\":\\"-\\"' "$MC_TEST_CARGO_ARGS"; then
  echo 'FAIL: 凭据齐全时仍向 tauri 传入 adhoc signingIdentity'
  exit 1
fi
if grep -q 'WARN: 跳过公证' "$fixture_notary/notary.log"; then
  echo 'FAIL: 凭据齐全时不应 WARN 跳过公证'
  exit 1
fi

echo 'PASS: 打包默认无冒烟、显式验收、失败传播、帮助与参数校验、dmg-only 挂载校验、公证路径'
