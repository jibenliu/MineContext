#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts/tests" "$fixture/scripts/lib" "$fixture/bin" "$fixture/tmp"
cp "$root/scripts/tests/launch-check-tauri.sh" "$fixture/scripts/tests/"
printf 'xtask_run() { return 0; }\n' > "$fixture/scripts/lib/xtask.sh"
cat > "$fixture/bin/hdiutil" <<'SH'
#!/usr/bin/env bash
if [ "$1" = attach ]; then
  mount="$4"
  mkdir -p "$mount/MineContext.app/Contents/MacOS" "$mount/MineContext.app/Contents/Resources/backend"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$mount/MineContext.app/Contents/Resources/backend/mc-daemon"
  cat > "$mount/MineContext.app/Contents/MacOS/minecontext-shell" <<'APP'
#!/usr/bin/env bash
mkdir -p "$MC_DATA_DIR/logs"
echo startup-error > "$MC_DATA_DIR/logs/daemon.log"
echo shell-error
exit 1
APP
  chmod +x "$mount/MineContext.app/Contents/MacOS/minecontext-shell" "$mount/MineContext.app/Contents/Resources/backend/mc-daemon"
fi
SH
chmod +x "$fixture/bin/hdiutil"
if PATH="$fixture/bin:$PATH" TMPDIR="$fixture/tmp" bash "$fixture/scripts/tests/launch-check-tauri.sh" fake.dmg > "$fixture/output" 2>&1; then
  echo 'FAIL: 外壳失败却返回成功'; exit 1
fi
grep -q shell-error "$fixture/output"
grep -q startup-error "$fixture/output"
grep -q '诊断目录已保留' "$fixture/output"
test "$(find "$fixture/tmp" -name daemon.log | wc -l | tr -d ' ')" = 1
echo 'PASS: 启动失败传播、外壳与后端日志展示、诊断目录保留'
