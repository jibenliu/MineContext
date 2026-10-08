#!/usr/bin/env bash
# 故障排查文档的新鲜度：文档里的错误码与文案必须与 `ErrorCode` 一致。
#
# 为什么值得一条门禁：新增一个错误码却忘了写进用户文档，用户遇到它时
# 只能看到一句人话文案，不知道该怎么办。这类漂移不会让任何测试变红，
# 只会在真实求助场景里暴露。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口：不需要系统 Python。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-troubleshooting-freshness
