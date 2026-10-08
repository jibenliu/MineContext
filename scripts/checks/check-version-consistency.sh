#!/usr/bin/env bash
# 版本号守卫：四处（Cargo workspace / 外壳 crate / package.json / tauri.conf.json）
# 必须是同一个版本，否则产物元数据、安装包名与前端包会各说各话。
#
# 检查逻辑在 `apps/xtask`（Rust），这里只是入口。
# 自检只覆盖「干净代码必须通过」：植入违规要改动版本文件，风险大于收益。
set -euo pipefail

cd "$(dirname "$0")/../.."

. "$(dirname "$0")/../lib/xtask.sh"

xtask_run check-version-consistency
