//! macOS 屏幕录制权限。
//!
//! 权限没拿到时 macOS **不会报错，而是返回全黑帧**。
//! 如果不主动检测，用户看到的是「一切正常但什么都没记录」——
//! 这类失败是「黑盒」的典型形态：没有报错，只有结果不对。

use mc_common::error::{AppError, ErrorCode};

use crate::source::PermissionState;

// 只声明两个函数，避免为一次权限查询引入整个 CoreGraphics 绑定。
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// 只查询、不弹窗
    fn CGPreflightScreenCaptureAccess() -> bool;
    /// 查询，必要时弹系统授权对话框
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// 查询当前权限状态（不弹窗）。
pub fn preflight() -> PermissionState {
    permission_from_preflight(unsafe { CGPreflightScreenCaptureAccess() })
}

/// 请求权限（会弹系统对话框）。返回请求后的状态。
pub fn request() -> PermissionState {
    permission_from_preflight(unsafe { CGRequestScreenCaptureAccess() })
}

/// 纯映射，便于在没有权限的机器上测试。
pub const fn permission_from_preflight(granted: bool) -> PermissionState {
    if granted {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

/// 权限缺失时的错误，带可执行建议（打开系统设置深链）。
pub fn permission_denied_error() -> AppError {
    AppError::new(
        ErrorCode::CapturePermissionDenied,
        "缺少屏幕录制权限（CGPreflightScreenCaptureAccess 返回 false）",
    )
    .with_context("platform", "macos")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_mapping_is_total() {
        assert_eq!(permission_from_preflight(true), PermissionState::Granted);
        assert_eq!(permission_from_preflight(false), PermissionState::Denied);
    }

    #[test]
    fn permission_error_carries_remediation() {
        let error = permission_denied_error();
        assert_eq!(error.code(), ErrorCode::CapturePermissionDenied);
        let remediation = error.remediation().expect("必须给出可执行建议");
        assert!(
            remediation
                .target
                .unwrap_or_default()
                .contains("systempreferences"),
            "建议应指向系统设置的屏幕录制面板"
        );
    }
}
