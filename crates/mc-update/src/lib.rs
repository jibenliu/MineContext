//! GitHub Releases 轻量更新检查：比对最新 tag 与本机版本，返回发布页 / dmg 链接。
//!
//! **纯解析与比较，无网络**——HTTP 拉取由外壳负责。不做静默下载或安装
//!（那需要签名与公证）。

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// 与历史 `window.api.checkForUpdate()` 对齐的更新摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub html_url: String,
    pub dmg_url: Option<String>,
}

/// 检查结果：无更新时 `update_info` 为 `None`（序列化为 `null`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckForUpdateResult {
    pub update_info: Option<UpdateInfo>,
    pub current_version: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// 去掉 tag 常见的 `v` / `V` 前缀，便于与 `CARGO_PKG_VERSION` 比较。
pub fn strip_v_prefix(tag: &str) -> &str {
    tag.strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag)
}

/// 解析 `major.minor.patch`（预发布后缀忽略）；无法解析的段按 0。
fn parse_semver_parts(version: &str) -> [u64; 3] {
    let core = version.split('-').next().unwrap_or(version);
    let mut parts = [0u64; 3];
    for (i, piece) in core.split('.').take(3).enumerate() {
        parts[i] = piece.parse().unwrap_or(0);
    }
    parts
}

/// `latest` 是否严格新于 `current`（均允许带 `v` 前缀）。
pub fn version_is_newer(latest: &str, current: &str) -> bool {
    let a = parse_semver_parts(strip_v_prefix(latest));
    let b = parse_semver_parts(strip_v_prefix(current));
    a.cmp(&b) == Ordering::Greater
}

/// 在资产里挑 macOS `.dmg`；同名多个时优先带 `aarch64` 的。
pub fn pick_dmg_url<N: AsRef<str>, U: AsRef<str>>(assets: &[(N, U)]) -> Option<String> {
    let mut fallback: Option<String> = None;
    for (name, url) in assets {
        let name = name.as_ref();
        if !name.to_ascii_lowercase().ends_with(".dmg") {
            continue;
        }
        let url = url.as_ref().to_string();
        if name.to_ascii_lowercase().contains("aarch64") {
            return Some(url);
        }
        if fallback.is_none() {
            fallback = Some(url);
        }
    }
    fallback
}

/// 用 Releases JSON 正文与本机版本算出检查结果（纯函数，便于单测）。
pub fn evaluate_latest_release(
    current_version: &str,
    body: &str,
) -> Result<CheckForUpdateResult, String> {
    let release: GithubRelease = serde_json::from_str(body)
        .map_err(|error| format!("解析 GitHub Releases 失败：{error}"))?;
    let latest = strip_v_prefix(&release.tag_name).to_string();
    let asset_pairs: Vec<(String, String)> = release
        .assets
        .iter()
        .map(|a| (a.name.clone(), a.browser_download_url.clone()))
        .collect();
    let dmg_url = pick_dmg_url(&asset_pairs);
    let update_info = if version_is_newer(&latest, current_version) {
        Some(UpdateInfo {
            version: latest,
            html_url: release.html_url,
            dmg_url,
        })
    } else {
        None
    };
    Ok(CheckForUpdateResult {
        update_info,
        current_version: current_version.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_NEWER: &str = r#"{
      "tag_name": "v1.0.8",
      "html_url": "https://github.com/jibenliu/MineContext/releases/tag/v1.0.8",
      "assets": [
        {
          "name": "darwin-aarch64.zip",
          "browser_download_url": "https://example.com/darwin-aarch64.zip"
        },
        {
          "name": "MineContext_1.0.8_aarch64.dmg",
          "browser_download_url": "https://example.com/MineContext_1.0.8_aarch64.dmg"
        }
      ]
    }"#;

    const SAMPLE_SAME: &str = r#"{
      "tag_name": "v1.0.7",
      "html_url": "https://github.com/jibenliu/MineContext/releases/tag/v1.0.7",
      "assets": [
        {
          "name": "MineContext_1.0.7_aarch64.dmg",
          "browser_download_url": "https://example.com/MineContext_1.0.7_aarch64.dmg"
        }
      ]
    }"#;

    #[test]
    fn strip_v_prefix_handles_common_tags() {
        assert_eq!(strip_v_prefix("v1.0.7"), "1.0.7");
        assert_eq!(strip_v_prefix("V2.0.0"), "2.0.0");
        assert_eq!(strip_v_prefix("1.2.3"), "1.2.3");
    }

    #[test]
    fn version_is_newer_compares_semver_core() {
        assert!(version_is_newer("1.0.8", "1.0.7"));
        assert!(version_is_newer("v1.1.0", "1.0.9"));
        assert!(!version_is_newer("1.0.7", "1.0.7"));
        assert!(!version_is_newer("v1.0.6", "1.0.7"));
    }

    #[test]
    fn pick_dmg_url_prefers_aarch64_dmg() {
        let assets = [
            ("MineContext_1.0.8_x64.dmg", "https://example.com/x64.dmg"),
            (
                "MineContext_1.0.8_aarch64.dmg",
                "https://example.com/arm.dmg",
            ),
        ];
        assert_eq!(
            pick_dmg_url(&assets).as_deref(),
            Some("https://example.com/arm.dmg")
        );
    }

    #[test]
    fn evaluate_latest_release_returns_update_when_newer() {
        let got = evaluate_latest_release("1.0.7", SAMPLE_NEWER).expect("parse");
        assert_eq!(got.current_version, "1.0.7");
        let info = got.update_info.expect("update");
        assert_eq!(info.version, "1.0.8");
        assert_eq!(
            info.html_url,
            "https://github.com/jibenliu/MineContext/releases/tag/v1.0.8"
        );
        assert_eq!(
            info.dmg_url.as_deref(),
            Some("https://example.com/MineContext_1.0.8_aarch64.dmg")
        );
    }

    #[test]
    fn evaluate_latest_release_null_when_current_is_latest() {
        let got = evaluate_latest_release("1.0.7", SAMPLE_SAME).expect("parse");
        assert!(got.update_info.is_none());
        assert_eq!(got.current_version, "1.0.7");
    }

    #[test]
    fn evaluate_latest_release_rejects_bad_json() {
        let err = evaluate_latest_release("1.0.7", "not-json").expect_err("bad json");
        assert!(err.contains("解析"), "{err}");
    }
}
