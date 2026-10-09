//! 平台相关实现。
//!
//! 规则：**跨平台代码里不出现 `#[cfg]`**，
//! `#[cfg]` 只出现在本模块与平台子目录内。

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(not(target_os = "macos"))]
pub mod unsupported;

/// 采集就绪度：给 `mc-cli doctor` 与首启引导用。
///
/// 保持平台无关，因此上层（CLI / 服务）不需要写 `#[cfg]`。
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureReadiness {
    pub available: bool,
    pub permission: super::source::PermissionState,
    pub monitor_count: usize,
    pub message: Option<String>,
}

/// 探测当前平台的采集就绪度（不弹窗）。
///
/// macOS 上若 `CGPreflight` 为 false，可能做一次**带超时**的交叉截屏以消除假阴性；
/// 超时或黑帧按缺权限处理，避免同步挂起拖死 daemon。
pub fn probe_readiness() -> CaptureReadiness {
    #[cfg(target_os = "macos")]
    {
        let permission = macos::permission::preflight();
        let monitor_count = xcap::Monitor::all().map(|m| m.len()).unwrap_or(0);
        let available = permission == super::source::PermissionState::Granted && monitor_count > 0;
        CaptureReadiness {
            available,
            permission,
            monitor_count,
            message: if available {
                None
            } else if permission == super::source::PermissionState::Denied {
                Some("缺少屏幕录制权限：请在「系统设置 → 隐私与安全性 → 屏幕录制」中勾选 MineContext 与 mc-daemon（或助手进程），然后完全退出并重开应用".to_string())
            } else {
                Some("没有检测到显示器".to_string())
            },
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        CaptureReadiness {
            available: false,
            permission: super::source::PermissionState::Unknown,
            monitor_count: 0,
            message: Some("当前平台尚未实现屏幕采集".to_string()),
        }
    }
}

/// 请求屏幕录制权限（macOS 可能弹系统对话框；其它平台返回 Unknown）。
pub fn request_permission() -> super::source::PermissionState {
    #[cfg(target_os = "macos")]
    {
        macos::permission::request()
    }
    #[cfg(not(target_os = "macos"))]
    {
        super::source::PermissionState::Unknown
    }
}

/// 打开系统设置的屏幕录制面板（非 macOS 为 no-op 成功）。
pub fn open_screen_recording_settings() -> Result<(), mc_common::error::AppError> {
    #[cfg(target_os = "macos")]
    {
        macos::permission::open_system_settings()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// 采集不可用时给出带 remediation 的错误。
///
/// 上层（CLI / HTTP / daemon）共用同一份构造逻辑，
/// 保证「缺权限」在任何入口都指向同一个解决路径。
pub fn readiness_error(readiness: &CaptureReadiness) -> mc_common::error::AppError {
    use super::source::PermissionState;
    use mc_common::error::{AppError, ErrorCode};

    let code = if readiness.permission == PermissionState::Denied {
        ErrorCode::CapturePermissionDenied
    } else {
        ErrorCode::CaptureNoDisplay
    };

    AppError::new(
        code,
        readiness
            .message
            .clone()
            .unwrap_or_else(|| "采集当前不可用".to_string()),
    )
}

/// 按配置选出的采集源集合。
pub struct SourceSelection {
    pub source: std::sync::Arc<dyn super::source::CaptureSource>,
    /// 配置中未能映射到当前平台的源 id。
    ///
    /// 单独列出来是为了让上层能**告诉用户**「该源在当前平台不可用」，
    /// 而不是让数据静静地缺失。
    pub ignored: Vec<String>,
}

impl SourceSelection {
    /// 本次真正启用的源 id（`CompositeSource` 会把子源按 `|` 串起来）。
    pub fn source_ids(&self) -> Vec<String> {
        self.source
            .id()
            .split('|')
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
    }
}

impl std::fmt::Debug for SourceSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceSelection")
            .field("source", &self.source.id())
            .field("ignored", &self.ignored)
            .finish()
    }
}

/// 配置里的 `capture.sources` → 真正要跑的采集源。
///
/// 规则：
/// - `"screen"` / `"window"` 在当前平台有实现；
/// - 其余（`clipboard` / `file` / `browser` 以及任何拼错的字符串）
///   记入 `ignored`，**不会**被悄悄替换成屏幕采集；
/// - 一个可用的源都没有时**直接报错**（fail-closed）：宁可采不了，
///   也不能在用户只勾了未实现源的情况下擅自扩大采集范围。
pub fn sources_for(ids: &[String]) -> Result<SourceSelection, mc_common::error::AppError> {
    platform_selection(ids)
}

#[cfg(target_os = "macos")]
fn platform_selection(ids: &[String]) -> Result<SourceSelection, mc_common::error::AppError> {
    let mut sources: Vec<std::sync::Arc<dyn super::source::CaptureSource>> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();

    for id in ids {
        match id.as_str() {
            "screen" => sources.push(std::sync::Arc::new(macos::MacScreenSource::new()?)),
            "window" => sources.push(std::sync::Arc::new(macos::window::MacWindowSource::new()?)),
            other => ignored.push(other.to_string()),
        }
    }

    if sources.is_empty() {
        let listed = if ignored.is_empty() {
            "（空）".to_string()
        } else {
            ignored.join("、")
        };
        return Err(mc_common::error::AppError::new(
            mc_common::error::ErrorCode::CaptureNoDisplay,
            format!("配置里的采集源都还没有实现：{listed}（可用：screen、window）"),
        ));
    }

    Ok(SourceSelection {
        source: std::sync::Arc::new(super::composite::CompositeSource::new(sources)),
        ignored,
    })
}

#[cfg(not(target_os = "macos"))]
fn platform_selection(ids: &[String]) -> Result<SourceSelection, mc_common::error::AppError> {
    if ids.is_empty() {
        return Err(mc_common::error::AppError::new(
            mc_common::error::ErrorCode::CaptureNoDisplay,
            "没有配置任何采集源".to_string(),
        ));
    }
    Ok(SourceSelection {
        source: std::sync::Arc::new(unsupported::UnsupportedScreenSource::new(
            std::env::consts::OS,
        )),
        ignored: ids.to_vec(),
    })
}
