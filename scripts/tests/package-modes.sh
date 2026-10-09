#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts/tests" "$fixture/frontend" "$fixture/src-tauri/target/release/bundle/dmg" "$fixture/bin"
cp "$root/scripts/package-macos-tauri.sh" "$fixture/scripts/"
touch "$fixture/src-tauri/target/release/bundle/dmg/test.dmg"
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
bash "$fixture/scripts/package-macos-tauri.sh" > "$fixture/default.log" 2>&1
test ! -s "$MC_TEST_SMOKE_LOG"
grep -q '未运行启动验收' "$fixture/default.log"
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
echo 'PASS: 打包默认无冒烟、显式验收、失败传播、帮助与参数校验'
