#!/usr/bin/env bash
# macos-notarize-env.sh：凭据齐全 / 缺项时的就绪判定必须可测。
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../lib/macos-notarize-env.sh
. "$root/scripts/lib/macos-notarize-env.sh"

clear_apple_env() {
  unset APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
    APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID \
    APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH APPLE_API_KEY_P8 \
    API_PRIVATE_KEYS_DIR
}

clear_apple_env
if mc_macos_notarize_ready; then
  echo 'FAIL: 空环境不应就绪'
  exit 1
fi
reason="$(mc_macos_notarize_skip_reason)"
if [ -z "$reason" ]; then
  echo 'FAIL: 空环境应给出 skip 原因'
  exit 1
fi

clear_apple_env
export APPLE_CERTIFICATE='Y2VydA=='
export APPLE_CERTIFICATE_PASSWORD='pw'
export APPLE_SIGNING_IDENTITY='Developer ID Application: Example (TEAMID)'
if mc_macos_notarize_ready; then
  echo 'FAIL: 仅有签名凭据、无公证认证时不应就绪'
  exit 1
fi
reason="$(mc_macos_notarize_skip_reason)"
case "$reason" in
  *公证* | *notar*) ;;
  *)
    echo "FAIL: 缺公证认证时应点明公证，实际：${reason}"
    exit 1
    ;;
esac

clear_apple_env
export APPLE_CERTIFICATE='Y2VydA=='
export APPLE_CERTIFICATE_PASSWORD='pw'
export APPLE_SIGNING_IDENTITY='Developer ID Application: Example (TEAMID)'
export APPLE_ID='dev@example.com'
export APPLE_PASSWORD='app-specific'
export APPLE_TEAM_ID='TEAMID1234'
if ! mc_macos_notarize_ready; then
  echo 'FAIL: Apple ID 三件套 + 签名凭据应就绪'
  exit 1
fi
if [ -n "$(mc_macos_notarize_skip_reason)" ]; then
  echo 'FAIL: 就绪时 skip 原因应为空'
  exit 1
fi

clear_apple_env
export APPLE_CERTIFICATE='Y2VydA=='
export APPLE_CERTIFICATE_PASSWORD='pw'
export APPLE_SIGNING_IDENTITY='Developer ID Application: Example (TEAMID)'
export APPLE_API_ISSUER='issuer-uuid'
export APPLE_API_KEY='KEYID'
export APPLE_API_KEY_PATH='/tmp/AuthKey_KEYID.p8'
if ! mc_macos_notarize_ready; then
  echo 'FAIL: API key 路径 + 签名凭据应就绪'
  exit 1
fi

clear_apple_env
export APPLE_CERTIFICATE='Y2VydA=='
export APPLE_CERTIFICATE_PASSWORD='pw'
export APPLE_SIGNING_IDENTITY='Developer ID Application: Example (TEAMID)'
export APPLE_API_ISSUER='issuer-uuid'
export APPLE_API_KEY='KEYID'
export APPLE_API_KEY_P8='-----BEGIN PRIVATE KEY-----
test
-----END PRIVATE KEY-----'
if ! mc_macos_notarize_ready; then
  echo 'FAIL: API key P8 内容 + 签名凭据应就绪'
  exit 1
fi

clear_apple_env
export APPLE_SIGNING_IDENTITY='-'
export APPLE_CERTIFICATE='Y2VydA=='
export APPLE_CERTIFICATE_PASSWORD='pw'
export APPLE_ID='dev@example.com'
export APPLE_PASSWORD='app-specific'
export APPLE_TEAM_ID='TEAMID1234'
if mc_macos_notarize_ready; then
  echo 'FAIL: signingIdentity "-" 不应算 Developer ID 就绪'
  exit 1
fi

clear_apple_env
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
export APPLE_API_ISSUER='issuer-uuid'
export APPLE_API_KEY='KEYID'
export APPLE_API_KEY_P8='-----BEGIN PRIVATE KEY-----
test
-----END PRIVATE KEY-----'
mc_macos_materialize_api_key_p8 "$tmpdir"
if [ ! -f "${APPLE_API_KEY_PATH:-}" ]; then
  echo 'FAIL: 应写出 APPLE_API_KEY_PATH'
  exit 1
fi
grep -q 'BEGIN PRIVATE KEY' "$APPLE_API_KEY_PATH"

echo 'PASS: macos-notarize-env 就绪判定与 P8 落盘'
