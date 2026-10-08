//! 平台支持范围：最低 **macOS 13（Ventura）**，同时覆盖 **macOS 14（Sonoma）**。
//!
//! 这条约束必须同时体现在三处，缺一处就会出现「文档说支持、二进制不支持」：
//!
//! 1. 构建：`MACOSX_DEPLOYMENT_TARGET`（`.cargo/config.toml`，由
//!    `scripts/check-macos-target.sh` 守卫）；
//! 2. 运行：本模块的判定，供 `/api/diagnostics` 与 `mc-cli doctor` 展示；
//! 3. CI：`macos-13` 与 `macos-14` 两个 runner 都要跑。
//!
//! 版本比较一律按数值（`13.10 > 13.9`），不做字符串比较。

use std::fmt;

use crate::error::{AppError, ErrorCode};

/// 最低支持的 macOS 版本。
pub const MIN_SUPPORTED_MACOS: MacOsVersion = MacOsVersion::new(13, 0);

/// 主版本 + 次版本。补丁号对我们没有影响，因此不保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacOsVersion {
    pub major: u32,
    pub minor: u32,
}

impl MacOsVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for MacOsVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// 解析 `sw_vers -productVersion` 的输出（`14.5` / `13.0` / `10.15.7`）。
///
/// 只取主次版本；补丁号与空白都容忍。解析不出来一律报错 ——
/// 猜一个版本号比报错更危险（会得出「支持」的错误结论）。
pub fn parse_sw_vers(raw: &str) -> Result<MacOsVersion, AppError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(invalid(raw));
    }

    let mut parts = text.split('.');
    let major = parse_component(parts.next(), raw)?;
    // `sw_vers` 可能只给主版本号
    let minor = match parts.next() {
        Some(value) => parse_component(Some(value), raw)?,
        None => 0,
    };

    Ok(MacOsVersion::new(major, minor))
}

fn parse_component(value: Option<&str>, raw: &str) -> Result<u32, AppError> {
    let value = value.ok_or_else(|| invalid(raw))?;
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(invalid(raw));
    }
    value.parse::<u32>().map_err(|_| invalid(raw))
}

fn invalid(raw: &str) -> AppError {
    AppError::new(
        ErrorCode::DomainInvalidRange,
        format!("无法解析系统版本 {raw:?}；期望形如 14.5 的版本号"),
    )
}

/// 该系统版本是否受支持。更高的版本按向上兼容处理。
pub fn is_supported(version: MacOsVersion) -> bool {
    version >= MIN_SUPPORTED_MACOS
}

/// 诊断用的一句话结论。
pub struct SupportReport {
    pub supported: bool,
    pub message: String,
}

pub fn support_report(version: MacOsVersion) -> SupportReport {
    if is_supported(version) {
        SupportReport {
            supported: true,
            message: format!("macOS {version}（最低要求 {MIN_SUPPORTED_MACOS}）"),
        }
    } else {
        SupportReport {
            supported: false,
            message: format!(
                "macOS {version} 不受支持：本应用最低要求 macOS {MIN_SUPPORTED_MACOS}"
            ),
        }
    }
}

/// 当前系统的版本（通过 `sw_vers` 读取）。
///
/// 非 macOS 平台返回 `None` —— 调用方据此跳过这项检查，而不是伪造一个版本号。
pub fn current() -> Option<MacOsVersion> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_sw_vers(&String::from_utf8_lossy(&output.stdout)).ok()
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_accepts_partial_versions() {
        assert_eq!(parse_sw_vers(" 13.6 ").unwrap(), MacOsVersion::new(13, 6));
        assert_eq!(parse_sw_vers("13").unwrap(), MacOsVersion::new(13, 0));
        // 三段版本只取前两段
        assert_eq!(parse_sw_vers("10.15.7").unwrap(), MacOsVersion::new(10, 15));
    }

    #[test]
    fn rejects_non_numeric_components() {
        assert!(parse_sw_vers("13.x").is_err());
        assert!(parse_sw_vers("-1.0").is_err());
        assert!(parse_sw_vers("13.").is_err());
    }
}
