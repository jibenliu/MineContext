#!/usr/bin/env bash
# 打当前平台的 release 二进制（mc-daemon + mc-cli），落到 dist/release/<platform>/。
#
# 用法：
#   ./scripts/package-release-binaries.sh
#   ./scripts/package-release-binaries.sh --out dist/release
#
# CI：`.github/workflows/release.yml` 在 macos / ubuntu / windows 矩阵里调本脚本；
# 推 `v*` 标签后自动跑，产物进 artifact，并由 draft GitHub Release 汇总。
set -euo pipefail

cd "$(dirname "$0")/.."

OUT="dist/release"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --out)
      shift
      OUT="${1:-}"
      if [ -z "$OUT" ]; then
        echo "错误：--out 需要目录参数" >&2
        exit 2
      fi
      ;;
    --out=*)
      OUT="${1#--out=}"
      ;;
    --help | -h)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *)
      echo "错误：未知参数 $1" >&2
      exit 2
      ;;
  esac
  shift
done

# 平台目录名：darwin-aarch64 / linux-x86_64 / windows-x86_64 …
uname_s="$(uname -s | tr '[:upper:]' '[:lower:]')"
uname_m="$(uname -m)"
case "$uname_s" in
  darwin) os="darwin" ;;
  linux) os="linux" ;;
  mingw* | msys* | cygwin*) os="windows" ;;
  *) os="$uname_s" ;;
esac
case "$uname_m" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) arch="$uname_m" ;;
esac
platform="${os}-${arch}"

echo "== 构建 release 二进制（${platform}） =="
cargo build --release -p mc-daemon -p mc-cli

dest="${OUT}/${platform}"
rm -rf "$dest"
mkdir -p "$dest"

ext=""
if [ "$os" = "windows" ]; then
  ext=".exe"
fi

for bin in mc-daemon mc-cli; do
  src="target/release/${bin}${ext}"
  if [ ! -f "$src" ]; then
    echo "FAIL: 缺少产物 ${src}" >&2
    exit 1
  fi
  cp "$src" "${dest}/${bin}${ext}"
  chmod +x "${dest}/${bin}${ext}" 2>/dev/null || true
done

(
  cd "$dest"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum ./*"${ext}" > SHA256SUMS
  else
    shasum -a 256 ./*"${ext}" > SHA256SUMS
  fi
)

echo "产物目录：${dest}"
ls -la "$dest"
echo "---- SHA256SUMS ----"
cat "${dest}/SHA256SUMS"
