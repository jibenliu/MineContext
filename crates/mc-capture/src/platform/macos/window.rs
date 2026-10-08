//! macOS 窗口采集（基于 `xcap`）。
//!
//! 存在的理由有两个，缺一不可：**隐私规则要靠它生效**
//! （`blocked_apps` / `blocked_window_patterns` 的判定依据是应用名与窗口标题，
//! 只截显示器的源这两项永远是 `None`，规则配了也不会命中）；**信息密度**
//! （窗口标题 + 应用名是廉价且高价值的上下文）。
//!
//! **只采前台窗口的元数据，不抓窗口图像**：给每个可见窗口都抓一张，存储与
//! 后续分析成本会直接乘以窗口数。它产出的是 `image = None` 的观测 ——
//! 像素由屏幕源负责。

use async_trait::async_trait;
use mc_common::error::AppError;
use xcap::Window;

use super::map_xcap_error;
use crate::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind, TargetBounds, TargetKind,
};

/// 采集源 id。稳定，会写进观测与幂等键。
pub const SOURCE_ID: &str = "macos:window";

pub struct MacWindowSource;

impl MacWindowSource {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }

    fn windows() -> Result<Vec<Window>, AppError> {
        Window::all().map_err(|e| map_xcap_error(&e.to_string()))
    }

    /// 窗口属性 → 采集目标。
    ///
    /// 抽成纯函数是为了在没有屏幕录制权限的机器（以及 CI）上也能测试映射：
    /// 应用名/标题的取值、最小化标记、几何信息都在这儿定死。
    fn target_of(window: &Window) -> Option<CaptureTarget> {
        let id = window.id().ok()?;
        let app_name = window.app_name().ok().filter(|s| !s.trim().is_empty());
        let title = window.title().ok().filter(|s| !s.trim().is_empty());
        let minimized = window.is_minimized().unwrap_or(false);
        let (width, height) = (
            window.width().unwrap_or_default(),
            window.height().unwrap_or_default(),
        );

        if !is_capturable(app_name.as_deref(), title.as_deref(), width, height) {
            return None;
        }

        let bounds = match (window.x(), window.y()) {
            (Ok(x), Ok(y)) if width > 0 && height > 0 => Some(TargetBounds {
                x: x as i64,
                y: y as i64,
                width,
                height,
            }),
            _ => None,
        };

        // 缩放倍率跟着窗口所在显示器走（Retina 上窗口像素是逻辑尺寸的 2 倍）
        let (display_id, scale_factor) = match window.current_monitor() {
            Ok(monitor) => (
                monitor.id().ok().map(|id| id.to_string()),
                monitor.scale_factor().unwrap_or(1.0),
            ),
            Err(_) => (None, 1.0),
        };

        Some(window_target(
            id,
            app_name,
            title,
            bounds,
            minimized,
            scale_factor,
        ))
        .map(|mut target| {
            target.display_id = display_id;
            target
        })
    }
}

/// 纯映射：窗口属性 → 采集目标。
pub fn window_target(
    window_id: u32,
    app_name: Option<String>,
    title: Option<String>,
    bounds: Option<TargetBounds>,
    is_minimized: bool,
    scale_factor: f32,
) -> CaptureTarget {
    let name = title
        .clone()
        .or_else(|| app_name.clone())
        .unwrap_or_else(|| format!("window-{window_id}"));

    CaptureTarget {
        id: format!("window-{window_id}"),
        name,
        kind: TargetKind::Window,
        scale_factor,
        display_id: None,
        window_id: Some(u64::from(window_id)),
        app_name,
        window_title: title,
        is_visible: !is_minimized,
        bounds,
    }
}

/// 这一轮要不要采这个窗口。
///
/// 三个条件缺一不可：**在前台**（抓的始终是活动窗口）、
/// 有身份信息（应用名或标题）、且被配置选中。
pub fn should_capture(is_focused: bool, capturable: bool, wanted: bool) -> bool {
    is_focused && capturable && wanted
}

/// 这个窗口值不值得采。
///
/// 系统里有大量没有标题、没有应用名的辅助窗口（菜单栏、浮层、零尺寸窗口），
/// 采集它们只会把存储和后续分析浪费掉。
pub fn is_capturable(app_name: Option<&str>, title: Option<&str>, width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    app_name.is_some() || title.is_some()
}

#[async_trait]
impl CaptureSource for MacWindowSource {
    fn id(&self) -> &str {
        SOURCE_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Window
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::WINDOW_METADATA
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        let windows = Self::windows()?;
        Ok(windows.iter().filter_map(Self::target_of).collect())
    }

    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        if super::permission::preflight() == PermissionState::Denied {
            return Err(super::permission::permission_denied_error());
        }

        let windows = Self::windows()?;
        let mut captures = Vec::new();

        for window in &windows {
            let Some(target) = Self::target_of(window) else {
                continue;
            };
            let focused = window.is_focused().unwrap_or(false);
            if !should_capture(focused, target.is_visible, ctx.wants(&target.id)) {
                continue;
            }

            // 元数据观测：没有图像，隐私判定与活动识别都靠应用名 + 标题
            captures.push(RawCapture::from_source(self, target, ctx.now, None, None));
        }

        Ok(captures)
    }

    async fn health(&self) -> SourceHealth {
        let permission = super::permission::preflight();
        let window_count = Self::windows().map(|w| w.len()).unwrap_or(0);
        let available = permission == PermissionState::Granted && window_count > 0;

        SourceHealth {
            available,
            permission,
            message: if available {
                None
            } else if permission == PermissionState::Denied {
                Some("缺少屏幕录制权限，窗口采集不可用".to_string())
            } else {
                Some("当前没有可采集的窗口".to_string())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_id_and_kind_are_stable() {
        let target = window_target(3, Some("App".to_string()), None, None, false, 1.0);
        assert_eq!(target.id, "window-3");
        assert_eq!(target.window_id, Some(3));
        assert_eq!(target.kind, TargetKind::Window);
    }

    #[test]
    fn only_the_focused_window_is_captured() {
        assert!(should_capture(true, true, true));
        assert!(!should_capture(false, true, true), "后台窗口不采");
        assert!(!should_capture(true, false, true), "没有身份信息的窗口不采");
        assert!(!should_capture(true, true, false), "没被配置选中的不采");
    }

    #[test]
    fn zero_sized_and_anonymous_windows_are_skipped() {
        assert!(!is_capturable(None, None, 800, 600), "没有身份信息的窗口");
        assert!(!is_capturable(Some("App"), Some("标题"), 0, 600), "零宽");
        assert!(!is_capturable(Some("App"), Some("标题"), 800, 0), "零高");
        assert!(is_capturable(Some("App"), None, 800, 600));
        assert!(is_capturable(None, Some("标题"), 800, 600));
    }
}
