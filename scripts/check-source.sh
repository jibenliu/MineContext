#!/usr/bin/env bash
# 源码类门禁：注释风格、依赖方向、时间用法、测试夹具、提示词内嵌、
# provider 纯度、阶段总结不变量、日志卫生、过程中间产物不进索引。
#
# 逐条打印 PASS/FAIL：合并之后仍然要能一眼看出是哪条红了。
set -uo pipefail

cd "$(dirname "$0")/.."

failed=0
run() {
  local name="$1"
  shift
  if "$@" >/tmp/check-source.$$.log 2>&1; then
    echo "PASS: ${name}"
  else
    echo "FAIL: ${name}"
    sed 's/^/      /' /tmp/check-source.$$.log | head -20
    failed=1
  fi
}

run "版本号一致（四处同一来源）" ./scripts/checks/check-version-consistency.sh
run "注释风格（交付向 + 长度棘轮）" ./scripts/checks/check-comment-style.sh
run "依赖方向（crate 分层）" ./scripts/checks/check-dep-direction.sh
run "时间用法（不许 naive datetime）" ./scripts/checks/check-no-naive-datetime.sh
run "测试夹具不进生产代码" ./scripts/checks/check-no-test-fixtures.sh
run "提示词内嵌（模板都接线了）" ./scripts/checks/check-prompt-embedded.sh
run "provider 纯净性（不许厂商 SDK）" ./scripts/checks/check-provider-purity.sh
run "阶段总结不变量" ./scripts/checks/check-summary-invariant.sh
run "日志卫生（不含内容/密钥/路径）" ./scripts/checks/check-log-redaction.sh
run "前端样式变量（可解析 + 跟随主题）" ./scripts/checks/check-theme-tokens.sh
run "过程中间产物不进索引" ./scripts/checks/check-no-iteration-artifacts.sh

rm -f /tmp/check-source.$$.log
if [ "${failed}" -ne 0 ]; then
  echo
  echo "源码类门禁未通过"
  exit 1
fi
echo
echo "源码类门禁全部通过"
