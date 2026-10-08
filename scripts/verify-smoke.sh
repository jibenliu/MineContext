#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
if [ $# -ne 0 ]; then
  echo "用法：$0（提交、打版本或最终交付前显式运行）" >&2
  exit 2
fi

cargo test -p mc-daemon --features process-smoke --test e2e
./scripts/checks/check-macos-artifacts.sh
./scripts/tests/smoke-daemon.sh
./scripts/tests/launch-check-tauri.sh
