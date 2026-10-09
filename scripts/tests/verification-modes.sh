#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts/lib" "$fixture/scripts/tests" "$fixture/scripts/checks" "$fixture/frontend" "$fixture/bin"
cp "$root/scripts/verify-all.sh" "$fixture/scripts/verify-all.sh"
cp "$root/scripts/verify-affected.sh" "$fixture/scripts/verify-affected.sh"
cp "$root/scripts/check-macos.sh" "$fixture/scripts/check-macos.sh"
cat > "$fixture/scripts/lib/xtask.sh" <<'SH'
xtask_run() { return 0; }
SH
for script in tests/selftest-lints.sh check-source.sh check-docs.sh check-tests.sh checks/check-shell-vars.sh checks/check-timeout-cmd.sh checks/check-macos-target.sh tests/rust-tests-parallel.sh; do
  printf '#!/usr/bin/env bash\nexit 0\n' > "$fixture/scripts/$script"
done
for script in verify-smoke.sh checks/check-macos-artifacts.sh tests/smoke-daemon.sh tests/launch-check-tauri.sh; do
  cat > "$fixture/scripts/$script" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$0" >> "$MC_TEST_SMOKE_LOG"
exit "${MC_TEST_SMOKE_EXIT:-0}"
SH
done
for command in cargo git npm npx; do
  cat > "$fixture/bin/$command" <<'SH'
#!/usr/bin/env bash
printf 'ℹ tests 1\nℹ pass 1\nℹ fail 0\n Test Files 1 passed\n Tests 1 passed\n'
SH
done
printf '#!/usr/bin/env bash\nexit 0\n' > "$fixture/bin/git"
chmod +x "$fixture/bin/"* "$fixture/scripts/"*.sh "$fixture/scripts/tests/"*.sh "$fixture/scripts/checks/"*.sh
export PATH="$fixture/bin:$PATH"
export MC_TEST_SMOKE_LOG="$fixture/smoke.log"
unset MC_RUN_SMOKE

bash "$fixture/scripts/verify-all.sh" > "$fixture/default.log" 2>&1
if [ -s "$MC_TEST_SMOKE_LOG" ]; then
  echo 'FAIL: 默认验证执行了冒烟或产物检查'
  exit 1
fi
grep -q 'SKIP.*冒烟' "$fixture/default.log"
bash "$fixture/scripts/verify-all.sh" --with-smoke > "$fixture/explicit.log" 2>&1
test -s "$MC_TEST_SMOKE_LOG"
if MC_TEST_SMOKE_EXIT=7 bash "$fixture/scripts/verify-all.sh" --with-smoke > "$fixture/failure.log" 2>&1; then
  echo 'FAIL: 显式冒烟失败没有阻断交付验证'
  exit 1
fi
if bash "$fixture/scripts/verify-all.sh" --unknown > /dev/null 2>&1; then
  echo 'FAIL: 未知参数没有被拒绝'
  exit 1
fi
: > "$MC_TEST_SMOKE_LOG"
bash "$fixture/scripts/verify-affected.sh" > "$fixture/affected-default.log" 2>&1
bash "$fixture/scripts/verify-affected.sh" --list --with-smoke > "$fixture/affected-list.log" 2>&1
test ! -s "$MC_TEST_SMOKE_LOG"
bash "$fixture/scripts/verify-affected.sh" --with-smoke > "$fixture/affected-explicit.log" 2>&1
test -s "$MC_TEST_SMOKE_LOG"
if MC_TEST_SMOKE_EXIT=7 bash "$fixture/scripts/verify-affected.sh" --with-smoke > "$fixture/affected-failure.log" 2>&1; then
  echo 'FAIL: 定向验证没有传播冒烟失败'
  exit 1
fi
echo 'PASS: 全量/定向默认无冒烟、显式冒烟、失败传播、预览不执行、参数校验'
