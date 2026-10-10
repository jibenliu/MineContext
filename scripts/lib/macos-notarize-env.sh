#!/usr/bin/env bash
# macOS Developer ID 签名 + 公证：检测 Tauri 2 所需环境变量是否齐全。
# 由 package-macos-tauri.sh 与自检脚本 source；不写死、不生成凭据。
#
# 签名（CI 导出证书）：APPLE_CERTIFICATE + APPLE_CERTIFICATE_PASSWORD +
#   APPLE_SIGNING_IDENTITY（非 "-"）
# 本机钥匙串：仅 APPLE_SIGNING_IDENTITY（非 "-"）亦可
# 公证认证二选一：
#   Apple ID：APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID
#   API key：APPLE_API_ISSUER + APPLE_API_KEY +（APPLE_API_KEY_PATH 或 APPLE_API_KEY_P8）

mc_macos_nonempty() {
  [ -n "${1:-}" ]
}

mc_macos_signing_identity_ready() {
  local identity="${APPLE_SIGNING_IDENTITY:-}"
  mc_macos_nonempty "$identity" && [ "$identity" != "-" ]
}

mc_macos_signing_certificate_ready() {
  mc_macos_nonempty "${APPLE_CERTIFICATE:-}" && mc_macos_nonempty "${APPLE_CERTIFICATE_PASSWORD:-}"
}

# Developer ID 可签名：本机 identity，或 CI 的 p12 + identity（identity 覆盖 conf 里的 adhoc "-"）。
mc_macos_signing_ready() {
  if mc_macos_signing_certificate_ready; then
    mc_macos_signing_identity_ready
    return $?
  fi
  mc_macos_signing_identity_ready
}

mc_macos_notarize_apple_id_ready() {
  mc_macos_nonempty "${APPLE_ID:-}" &&
    mc_macos_nonempty "${APPLE_PASSWORD:-}" &&
    mc_macos_nonempty "${APPLE_TEAM_ID:-}"
}

mc_macos_notarize_api_key_ready() {
  mc_macos_nonempty "${APPLE_API_ISSUER:-}" &&
    mc_macos_nonempty "${APPLE_API_KEY:-}" &&
    {
      mc_macos_nonempty "${APPLE_API_KEY_PATH:-}" || mc_macos_nonempty "${APPLE_API_KEY_P8:-}"
    }
}

mc_macos_notarize_auth_ready() {
  mc_macos_notarize_apple_id_ready || mc_macos_notarize_api_key_ready
}

mc_macos_notarize_ready() {
  mc_macos_signing_ready && mc_macos_notarize_auth_ready
}

# 就绪时打印空行；否则一行中文原因（供 WARN）。
mc_macos_notarize_skip_reason() {
  if mc_macos_notarize_ready; then
    printf ''
    return 0
  fi
  if ! mc_macos_signing_ready; then
    printf '%s\n' '缺少 Developer ID 签名凭据（需 APPLE_CERTIFICATE + APPLE_CERTIFICATE_PASSWORD + APPLE_SIGNING_IDENTITY，或本机非 "-" 的 APPLE_SIGNING_IDENTITY）'
    return 0
  fi
  printf '%s\n' '缺少公证认证（Apple ID：APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID；或 API key：APPLE_API_ISSUER + APPLE_API_KEY + APPLE_API_KEY_PATH/APPLE_API_KEY_P8）'
}

# 若提供 APPLE_API_KEY_P8，写入 AuthKey_<id>.p8 并 export APPLE_API_KEY_PATH。
# $1 = 目录（默认 ${RUNNER_TEMP:-/tmp}/minecontext-notarize）
mc_macos_materialize_api_key_p8() {
  local dir="${1:-${RUNNER_TEMP:-/tmp}/minecontext-notarize}"
  if ! mc_macos_nonempty "${APPLE_API_KEY_P8:-}"; then
    return 0
  fi
  if ! mc_macos_nonempty "${APPLE_API_KEY:-}"; then
    echo 'WARN: 已设 APPLE_API_KEY_P8 但缺少 APPLE_API_KEY，无法落盘 .p8' >&2
    return 1
  fi
  mkdir -p "$dir"
  local path="$dir/AuthKey_${APPLE_API_KEY}.p8"
  # 不把密钥回显到日志；仅写文件。
  printf '%s\n' "$APPLE_API_KEY_P8" >"$path"
  chmod 600 "$path"
  export APPLE_API_KEY_PATH="$path"
}
