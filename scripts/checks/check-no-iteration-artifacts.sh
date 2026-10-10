#!/usr/bin/env bash
# 本地过程留档路径不得出现在 git 索引（约定见根目录 AGENTS.md 第 3 节）。
#
# ignore 挡住日常误加；本守卫拦住索引里已经存在的遗留路径。
# 只查 `git ls-files`；工作区未跟踪的本机留档是允许的。
set -euo pipefail

cd "$(dirname "$0")/../.."

# pathspec 与 .gitignore 的「本机留档」段对齐；改 ignore 时同步这里。
pathspecs=(
  'docs/internal'
  'docs/internal/**'
  'docs/external-verification.md'
  'docs/release-checklist.md'
  'docs/delivery-notes.md'
  'todo.md'
  '**/tdd-log.md'
  'tdd-log.md'
  '*.orig'
  '**/*.orig'
  '*.bak'
  '**/*.bak'
  '*~'
  '**/*~'
  'docs/*-wip.md'
  'docs/*-audit.md'
  'docs/*-plan.md'
  'docs/cleanup*.md'
  'docs/improvement*.md'
  '.cursor'
  '.cursor/**'
  '.codex'
  '.codex/**'
  '.agent'
  '.agent/**'
  'agent-scratch'
  'agent-scratch/**'
  '**/__agent_scratch__/**'
  'cursor/stores'
  'cursor/stores/**'
)

tracked="$(git ls-files -- "${pathspecs[@]}" 2>/dev/null || true)"
if [ -z "${tracked}" ]; then
  echo "PASS: 无本地过程留档路径被跟踪"
  exit 0
fi

echo "FAIL: 下列路径是本地过程留档，不应被 git 跟踪（见 AGENTS.md 第 3 节）："
printf '%s\n' "$tracked" | sed 's/^/  /'
exit 1
