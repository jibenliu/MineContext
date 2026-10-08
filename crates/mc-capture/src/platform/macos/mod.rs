//! macOS 屏幕采集（基于 `xcap`）。
//!
//! 设计取舍：用 `xcap` 而不是直接调 `CGDisplayCreateImage`。
//! 好处是同一套代码可覆盖 Windows/Linux，且它已经处理了
//! Retina 缩放与多显示器；代价是多一层依赖。
//!
//! **权限是这里最关键的部分**：macOS 在权限缺失时不会报错，而是返回全黑帧。
//! 因此 `poll` 会主动查权限，并且下游还有黑帧检测兜底。

pub mod permission;
pub mod window;

use async_trait::async_trait;
use mc_common::error::{AppError, ErrorCode};
use xcap::Monitor;

use crate::source::{
    CaptureContext, CaptureSource, CaptureTarget, RawCapture, SourceCapabilities, SourceHealth,
    SourceKind, TargetBounds, TargetKind,
};

/// 采集源 id。稳定，会写进观测与幂等键。
pub const SOURCE_ID: &str = "macos:screen";

pub struct MacScreenSource;

impl MacScreenSource {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }

    /// 当前权限状态（不弹窗）。
    pub fn permission(&self) -> crate::source::PermissionState {
        permission::preflight()
    }

    fn monitors() -> Result<Vec<Monitor>, AppError> {
        Monitor::all().map_err(|e| map_xcap_error(&e.to_string()))
    }
}

fn target_from_monitor(monitor: &Monitor) -> Result<CaptureTarget, AppError> {
    let id = monitor.id().map_err(|e| map_xcap_error(&e.to_string()))?;
    let name = monitor.name().unwrap_or_else(|_| format!("Display {id}"));
    let scale = monitor.scale_factor().unwrap_or(1.0);

    // 几何信息用于区域采集：拿不到时留空，由采集环降级为整屏采集
    let bounds = match (monitor.x(), monitor.y(), monitor.width(), monitor.height()) {
        (Ok(x), Ok(y), Ok(width), Ok(height)) => Some(TargetBounds {
            x: x as i64,
            y: y as i64,
            width,
            height,
        }),
        _ => None,
    };

    Ok(CaptureTarget {
        id: format!("display-{id}"),
        name,
        kind: TargetKind::Screen,
        scale_factor: scale,
        display_id: Some(id.to_string()),
        window_id: None,
        app_name: None,
        window_title: None,
        is_visible: true,
        bounds,
    })
}

/// `xcap` 的错误文本 → typed error。
///
/// 抽成纯函数是为了在没有权限的机器（以及 CI）上也能测试映射逻辑 ——
/// 真机权限路径由 `#[ignore]` 的手工测试覆盖。
pub fn map_xcap_error(message: &str) -> AppError {
    let lower = message.to_ascii_lowercase();

    let code = if lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("not authorized")
        || lower.contains("tcc")
    {
        ErrorCode::CapturePermissionDenied
    } else if lower.contains("monitor") || lower.contains("display") || lower.contains("no screen")
    {
        ErrorCode::CaptureNoDisplay
    } else if lower.contains("timeout") || lower.contains("timed out") {
        ErrorCode::CaptureTimeout
    } else {
        ErrorCode::CaptureIo
    };

    AppError::new(code, format!("屏幕采集失败: {message}")).with_context("platform", "macos")
}

/// RGBA → RGB。抽出来是因为它是每帧都要跑的热路径，
/// 而且与平台无关，值得单独测。
pub fn rgba_to_rgb(width: u32, height: u32, rgba: &[u8]) -> Result<image::RgbImage, AppError> {
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() < expected {
        return Err(AppError::new(
            ErrorCode::CaptureIo,
            format!(
                "帧缓冲区大小不符：期望至少 {expected} 字节，实际 {}",
                rgba.len()
            ),
        ));
    }

    // 直接在缓冲区上操作，避免 600 万次逐像素索引
    let mut buffer = Vec::with_capacity((width as usize) * (height as usize) * 3);
    for chunk in rgba[..expected].chunks_exact(4) {
        buffer.push(chunk[0]);
        buffer.push(chunk[1]);
        buffer.push(chunk[2]);
    }

    image::RgbImage::from_raw(width, height, buffer).ok_or_else(|| {
        AppError::new(
            ErrorCode::CaptureIo,
            "无法从缓冲区构造 RGB 图像（尺寸不匹配）".to_string(),
        )
    })
}

#[async_trait]
impl CaptureSource for MacScreenSource {
    fn id(&self) -> &str {
        SOURCE_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Screen
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::SCREEN
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        let monitors = Self::monitors()?;
        monitors.iter().map(target_from_monitor).collect()
    }

    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        // 权限缺失时 macOS 会返回全黑帧；主动查一次，给出可执行的错误而不是黑图
        if permission::preflight() == crate::source::PermissionState::Denied {
            return Err(permission::permission_denied_error());
        }

        let monitors = Self::monitors()?;
        if monitors.is_empty() {
            // 无显示器是正常状态（远程会话/合盖），不是错误
            return Ok(Vec::new());
        }

        let mut captures = Vec::new();
        for monitor in &monitors {
            let target = match target_from_monitor(monitor) {
                Ok(target) => target,
                Err(_) => continue,
            };
            if !ctx.wants(&target.id) {
                continue;
            }

            let frame = monitor
                .capture_image()
                .map_err(|e| map_xcap_error(&e.to_string()))?;

            let image = rgba_to_rgb(frame.width(), frame.height(), frame.as_raw())?;

            captures.push(RawCapture::from_source(
                self,
                target,
                ctx.now,
                Some(image),
                None,
            ));
        }

        Ok(captures)
    }

    async fn health(&self) -> SourceHealth {
        let permission = permission::preflight();
        let monitor_count = Self::monitors().map(|m| m.len()).unwrap_or(0);

        let available = permission == crate::source::PermissionState::Granted && monitor_count > 0;

        SourceHealth {
            available,
            permission,
            message: if available {
                None
            } else if permission == crate::source::PermissionState::Denied {
                Some("缺少屏幕录制权限".to_string())
            } else {
                Some("没有检测到显示器".to_string())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_are_mapped_to_permission_code() {
        for message in [
            "screen recording permission denied",
            "not authorized to capture",
            "TCC denied",
        ] {
            assert_eq!(
                map_xcap_error(message).code(),
                ErrorCode::CapturePermissionDenied,
                "应当识别为权限问题: {message}"
            );
            assert!(
                map_xcap_error(message).remediation().is_some(),
                "权限错误必须给出可执行建议"
            );
        }
    }

    #[test]
    fn display_errors_are_mapped_to_no_display() {
        assert_eq!(
            map_xcap_error("no monitor found").code(),
            ErrorCode::CaptureNoDisplay
        );
    }

    #[test]
    fn unknown_errors_fall_back_to_io() {
        assert_eq!(
            map_xcap_error("something weird happened").code(),
            ErrorCode::CaptureIo
        );
    }

    #[test]
    fn rgba_to_rgb_converts_correctly() {
        let rgb = rgba_to_rgb(2, 1, &[10, 20, 30, 255, 40, 50, 60, 128]).unwrap();
        assert_eq!(rgb.width(), 2);
        assert_eq!(rgb.height(), 1);
        assert_eq!(rgb.get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(rgb.get_pixel(1, 0).0, [40, 50, 60]);
    }

    #[test]
    fn rgba_to_rgb_rejects_short_buffer() {
        let error = rgba_to_rgb(2, 2, &[0, 0, 0, 255]).unwrap_err();
        assert_eq!(error.code(), ErrorCode::CaptureIo);
    }
}
