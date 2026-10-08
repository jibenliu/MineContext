#!/usr/bin/env bash
# 生产代码里不得出现测试夹具。
#
# 为什么值得一条独立门禁：夹具值（固定时间基准、示例文案）看起来无害，
# 一旦漏进 `src/**` 就会成为业务行为的一部分 —— 例如某处「默认活动标题」
# 变成了测试用的 `活动 xxx`，而它在测试里永远不会被发现。
#
# 这条守卫同时是「夹具集中到 mc-testkit」的推动力：测试需要这些值时
# 应当引用 `mc_testkit::fixtures::*`，而不是各写一份字面量。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-no-test-fixtures
