#!/usr/bin/env bash
# 打发版标签：读仓库版本（或入参），校验四处版本一致，创建并推送 `vX.Y.Z`。
#
# 用法：
#   ./scripts/create-release-tag.sh                 # 用 Cargo.toml workspace 版本
#   ./scripts/create-release-tag.sh 0.1.6           # 指定版本（可带或不带 v）
#   ./scripts/create-release-tag.sh --dry-run       # 只打印将要打的标签
#   ./scripts/create-release-tag.sh --local 0.1.6   # 只在本机建 tag，不 push
#   ./scripts/create-release-tag.sh --assert-only [VERSION]
#       # 只跑版本闸门（四处一致，可选等于 VERSION/tag）；不建 tag。Release CI 用。
#
# CI：`.github/workflows/tag-release.yml` 调本脚本；`release.yml` 在打包前
# `--assert-only` 对照 tag；推送 `v*` 后由 release.yml 接手二进制 + dmg + draft Release。
set -euo pipefail

cd "$(dirname "$0")/.."

. "$(dirname "$0")/lib/version-gate.sh"

DRY_RUN=0
LOCAL_ONLY=0
ASSERT_ONLY=0
VERSION_ARG=""

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --local) LOCAL_ONLY=1 ;;
    --assert-only) ASSERT_ONLY=1 ;;
    -h | --help)
      sed -n '2,13p' "$0"
      exit 0
      ;;
    -*)
      echo "未知参数：$arg" >&2
      exit 2
      ;;
    *)
      if [ -n "$VERSION_ARG" ]; then
        echo "版本只能指定一次" >&2
        exit 2
      fi
      VERSION_ARG="$arg"
      ;;
  esac
done

if [ "$ASSERT_ONLY" -eq 1 ]; then
  if [ -n "$VERSION_ARG" ]; then
    VERSION="${VERSION_ARG#v}"
    if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
      echo "非法版本号：${VERSION}（需要 SemVer，例如 0.1.5 或 v0.1.5）" >&2
      exit 1
    fi
    workspace_version="$(version_gate_assert "$VERSION")"
  else
    workspace_version="$(version_gate_assert)"
  fi
  echo "版本闸门通过（仓库版本 ${workspace_version}${VERSION_ARG:+，期望 ${VERSION_ARG#v}}）"
  exit 0
fi

workspace_version="$(version_gate_assert)"

if [ -n "$VERSION_ARG" ]; then
  VERSION="${VERSION_ARG#v}"
else
  VERSION="$workspace_version"
fi

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
  echo "非法版本号：${VERSION}（需要 SemVer，例如 0.1.5 或 v0.1.5）" >&2
  exit 1
fi

if [ "$VERSION" != "$workspace_version" ]; then
  echo "入参版本 ${VERSION} 与仓库版本 ${workspace_version} 不一致。" >&2
  echo "先改齐 Cargo.toml / src-tauri / frontend / tauri.conf 再打标签。" >&2
  exit 1
fi

TAG="v${VERSION}"

git fetch --tags --force origin 2>/dev/null || true
if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null 2>&1; then
  echo "标签已存在：${TAG}" >&2
  exit 1
fi
if git ls-remote --exit-code --tags origin "refs/tags/${TAG}" >/dev/null 2>&1; then
  echo "远端已有标签：${TAG}" >&2
  exit 1
fi

echo "将创建标签 ${TAG}（仓库版本 ${workspace_version}）→ $(git rev-parse --short HEAD)"

if [ "$DRY_RUN" -eq 1 ]; then
  echo "dry-run：未创建、未推送"
  exit 0
fi

git tag -a "$TAG" -m "Release ${TAG}"

if [ "$LOCAL_ONLY" -eq 1 ]; then
  echo "已在本机创建 ${TAG}（未推送）"
  exit 0
fi

git push origin "refs/tags/${TAG}"
echo "已推送 ${TAG}；Release workflow 应由 tag 推送触发（多平台二进制 + macOS dmg）。"
