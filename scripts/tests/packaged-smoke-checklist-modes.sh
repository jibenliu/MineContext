#!/usr/bin/env bash
# packaged-smoke-checklist.sh：默认只打印清单、不跑冒烟；--auto 跑可自动化项并传播失败。
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT

mkdir -p "$fixture/scripts/tests" \
  "$fixture/src-tauri/target/release/bundle/dmg" \
  "$fixture/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend" \
  "$fixture/bin" \
  "$fixture/docs/internal"

cp "$root/scripts/tests/packaged-smoke-checklist.sh" "$fixture/scripts/tests/"
chmod +x "$fixture/scripts/tests/packaged-smoke-checklist.sh"

# stub：记录是否被调用
cat > "$fixture/scripts/tests/launch-check-tauri.sh" <<'SH'
#!/usr/bin/env bash
echo launch-check >> "$MC_TEST_AUTO_LOG"
exit "${MC_TEST_LAUNCH_EXIT:-0}"
SH
cat > "$fixture/scripts/tests/verify-packaged-app.sh" <<'SH'
#!/usr/bin/env bash
echo verify-packaged >> "$MC_TEST_AUTO_LOG"
exit "${MC_TEST_PACKAGED_EXIT:-0}"
SH
chmod +x "$fixture/scripts/tests/launch-check-tauri.sh" \
  "$fixture/scripts/tests/verify-packaged-app.sh"

export MC_TEST_AUTO_LOG="$fixture/auto.log"
: > "$MC_TEST_AUTO_LOG"
cd "$fixture"

# --help
bash ./scripts/tests/packaged-smoke-checklist.sh --help > "$fixture/help.log"
grep -q -- '--auto' "$fixture/help.log"
grep -q -- '--record' "$fixture/help.log"

# 默认：打印清单且不跑自动化脚本
bash ./scripts/tests/packaged-smoke-checklist.sh > "$fixture/default.log"
test ! -s "$MC_TEST_AUTO_LOG"
for needle in '打包' '录制' '助手' '文件' '链接'; do
  grep -q "$needle" "$fixture/default.log" || {
    echo "FAIL: 默认清单缺少「${needle}」"
    exit 1
  }
done
# 发版路径提示：不应暗示日常提交要跑完整清单
if grep -Eqi 'verify-affected\.sh[^\n]*packaged-smoke|每次提交' "$fixture/default.log"; then
  echo 'FAIL: 清单把打包冒烟绑进日常提交路径'
  exit 1
fi

# 无产物时 --auto 必须失败（发版前不能 SKIP 当通过）
if bash ./scripts/tests/packaged-smoke-checklist.sh --auto > "$fixture/no-dmg.log" 2>&1; then
  echo 'FAIL: 无 dmg 时 --auto 却返回成功'
  exit 1
fi
grep -Eqi 'FAIL|没有|缺少|package-macos-tauri' "$fixture/no-dmg.log"
test ! -s "$MC_TEST_AUTO_LOG"

# 有产物时 --auto 跑 launch-check 与 verify-packaged-app
touch "$fixture/src-tauri/target/release/bundle/dmg/MineContext_0.0.0_test.dmg"
: > "$fixture/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend/mc-daemon"
chmod +x "$fixture/src-tauri/target/release/bundle/macos/MineContext.app/Contents/Resources/backend/mc-daemon"
: > "$MC_TEST_AUTO_LOG"
bash ./scripts/tests/packaged-smoke-checklist.sh --auto > "$fixture/auto-ok.log"
grep -q 'launch-check' "$MC_TEST_AUTO_LOG"
grep -q 'verify-packaged' "$MC_TEST_AUTO_LOG"
grep -q '录制' "$fixture/auto-ok.log"
grep -q '助手' "$fixture/auto-ok.log"

# launch-check 失败必须阻断
: > "$MC_TEST_AUTO_LOG"
if MC_TEST_LAUNCH_EXIT=7 bash ./scripts/tests/packaged-smoke-checklist.sh --auto > "$fixture/auto-fail.log" 2>&1; then
  echo 'FAIL: launch-check 失败却返回成功'
  exit 1
fi
grep -q 'launch-check' "$MC_TEST_AUTO_LOG"

# 未知参数拒绝
if bash ./scripts/tests/packaged-smoke-checklist.sh --unknown > /dev/null 2>&1; then
  echo 'FAIL: 未知参数没有被拒绝'
  exit 1
fi

# --record：管道输入写入本机留档（不入库路径）
: > "$MC_TEST_AUTO_LOG"
printf 'y\nok\ny\nok\ny\nok\n' | bash ./scripts/tests/packaged-smoke-checklist.sh --record > "$fixture/record.log"
record_file="$(ls -t "$fixture/docs/internal"/packaged-smoke-*.md | head -1)"
test -n "$record_file"
grep -q '录制' "$record_file"
grep -q '助手' "$record_file"
grep -Eq '文件|链接' "$record_file"
# --record 默认不跑自动化（避免日常误触发真机冒烟）
test ! -s "$MC_TEST_AUTO_LOG"

echo 'PASS: packaged-smoke-checklist 默认打印、--auto 产物门禁、失败传播、--record 留档'
