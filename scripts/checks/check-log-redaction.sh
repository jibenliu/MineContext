#!/usr/bin/env bash
# 日志卫生：日志里**不许出现内容、密钥、路径**，也不许绕过统一出口。
#
# 三条规则：
#   1. 只能用 `mc_common::observability` 的宏，不许直接调 `tracing::info!`
#      （绕过出口就绕过了脱敏策略）；
#   2. 字段名与格式化参数里不许出现 content / prompt / token / path / dir 这类
#      值，也不许 `.display()`（路径）与 `error.detail()`（未脱敏原文）；
#   3. 每个 crate 要么有日志点，要么在 `scripts/log-coverage.txt` 里写明理由。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-log-redaction
