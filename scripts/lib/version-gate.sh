#!/usr/bin/env bash
# 发版版本闸门：Cargo workspace / 外壳 crate / frontend / tauri.conf 四处必须一致，
# 且可选地等于期望 tag/版本（防止打出 v1.0.7 而字符串仍是 1.0.6）。
#
# 用法（被 source）：
#   . scripts/lib/version-gate.sh
#   version_gate_assert                 # 只校验四处一致 → 打印仓库版本到 stdout
#   version_gate_assert 1.0.7           # 四处一致且等于 1.0.7（可带或不带 v）
#   version_gate_assert v1.0.7
#
# 入口：`./scripts/create-release-tag.sh --assert-only [VERSION]`（Release CI 用）。
# shellcheck shell=bash

version_gate_read_cargo() {
  # 取文件里第一处顶格 `version = "..."`（workspace / crate 清单）
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$1" | head -1
}

version_gate_read_json() {
  # 不引 jq：用 python 读顶层 version（CI / 本机都有）
  python3 - "$1" <<'PY'
import json, sys
path = sys.argv[1]
with open(path, encoding="utf-8") as fh:
    data = json.load(fh)
version = data.get("version")
if not isinstance(version, str) or not version:
    raise SystemExit(f"{path} 里没有 version 字段")
print(version)
PY
}

# 断言四处一致；若传入期望版本则再断言等于它。成功时把仓库版本打印到 stdout。
version_gate_assert() {
  local expected="${1:-}"
  expected="${expected#v}"

  local workspace_version tauri_crate_version frontend_version tauri_conf_version
  workspace_version="$(version_gate_read_cargo Cargo.toml)"
  tauri_crate_version="$(version_gate_read_cargo src-tauri/Cargo.toml)"
  frontend_version="$(version_gate_read_json frontend/package.json)"
  tauri_conf_version="$(version_gate_read_json src-tauri/tauri.conf.json)"

  if [ -z "$workspace_version" ]; then
    echo "读不到 Cargo.toml 的 workspace version" >&2
    return 1
  fi

  local mismatched=0
  local pair source value
  for pair in \
    "src-tauri/Cargo.toml:${tauri_crate_version}" \
    "frontend/package.json:${frontend_version}" \
    "src-tauri/tauri.conf.json:${tauri_conf_version}"; do
    source="${pair%%:*}"
    value="${pair#*:}"
    if [ "$value" != "$workspace_version" ]; then
      echo "版本不一致：Cargo.toml = ${workspace_version}，但 ${source} = ${value}" >&2
      mismatched=1
    fi
  done
  if [ "$mismatched" -ne 0 ]; then
    return 1
  fi

  if [ -n "$expected" ] && [ "$expected" != "$workspace_version" ]; then
    echo "入参/标签版本 ${expected} 与仓库版本 ${workspace_version} 不一致。" >&2
    echo "先改齐 Cargo.toml / src-tauri / frontend / tauri.conf 再打标签或发版。" >&2
    return 1
  fi

  echo "$workspace_version"
}
