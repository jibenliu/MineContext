//! 外壳侧更新检查：拉 GitHub Releases，再用 `mc-update` 做纯比较。
//!
//! 不做静默下载或安装——那需要签名与公证；这里只回答「有没有更新」以及去哪打开。

use mc_update::{evaluate_latest_release, CheckForUpdateResult};

const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/jibenliu/MineContext/releases/latest";

/// 拉取 GitHub `releases/latest` 并与本机版本比较。
pub fn fetch_and_evaluate(current_version: &str) -> Result<CheckForUpdateResult, String> {
    let body = ureq::get(RELEASES_LATEST_URL)
        .set("User-Agent", "MineContext")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("请求 GitHub Releases 失败：{error}"))?
        .into_string()
        .map_err(|error| format!("读取 GitHub Releases 响应失败：{error}"))?;
    evaluate_latest_release(current_version, &body)
}

/// 用系统默认方式打开 http(s) 链接（发布页 / dmg）。
pub fn open_external_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("只允许打开 http(s) 链接".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|error| format!("打开链接失败：{error}"))?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .map_err(|error| format!("打开链接失败：{error}"))?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        Err("当前平台不支持打开外部链接".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_external_url_rejects_non_http() {
        let err = open_external_url("file:///etc/passwd").expect_err("scheme");
        assert!(err.contains("http"), "{err}");
    }
}
