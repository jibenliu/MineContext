//! macOS 屏幕录制权限。
//!
//! 权限没拿到时 macOS **不会报错，而是返回全黑帧**。
//! 如果不主动检测，用户看到的是「一切正常但什么都没记录」——
//! 这类失败是「黑盒」的典型形态：没有报错，只有结果不对。
//!
//! `CGPreflightScreenCaptureAccess` 只反映**当前进程**的 TCC 状态。
//! 采集跑在 `mc-daemon` 里，而系统设置里勾选的常常是外壳 `MineContext.app`：
//! Preflight 可能对已授权的采集能力返回 false（错 bundle / 授权后未刷新）。
//! 因此 Preflight 为 false 时再用一次轻量截屏交叉验证，避免假「缺少权限」。
//!
//! 交叉截屏必须带超时：部分 macOS 版本在无权限时 `capture_image` 会长时间挂起，
//! 若在 HTTP/采集热路径上同步等待，整 app 会表现为卡死且重启无效。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use mc_common::error::{AppError, ErrorCode};

use crate::change::{rgb_to_luma, ChangeDetector, DHashDetector, HashPolicy};
use crate::permission_resolve::{
    empirical_cache_ttl_secs, empirical_from_worker_timeout, resolve_screen_permission,
    EmpiricalCapture,
};
use crate::source::PermissionState;

use super::rgba_to_rgb;

// 只声明两个函数，避免为一次权限查询引入整个 CoreGraphics 绑定。
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// 只查询、不弹窗
    fn CGPreflightScreenCaptureAccess() -> bool;
    /// 查询，必要时弹系统对话框
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// 交叉截屏最长等待。超时按缺权限处理，避免拖死 daemon。
pub const EMPIRICAL_CAPTURE_TIMEOUT: Duration = Duration::from_secs(2);

/// 查询当前权限状态（不弹窗）。
///
/// Preflight 为 false 时会做一次带短缓存与超时的截屏交叉验证，消除「系统设置已开、
/// UI 仍报缺权限」的假阴性；超时则 fail-closed。
pub fn preflight() -> PermissionState {
    let preflight_granted = unsafe { CGPreflightScreenCaptureAccess() };
    if preflight_granted {
        return PermissionState::Granted;
    }
    resolve_screen_permission(false, empirical_capture_cached())
}

/// 请求权限（会弹系统对话框；若系统设置里已勾选则通常不弹）。返回请求后的状态。
pub fn request() -> PermissionState {
    let granted = unsafe { CGRequestScreenCaptureAccess() };
    // 请求后清掉经验证缓存，避免沿用请求前的否决结果。
    invalidate_empirical_cache();
    if granted {
        return PermissionState::Granted;
    }
    resolve_screen_permission(false, empirical_capture_cached())
}

/// 纯映射，便于在没有权限的机器上测试。
pub const fn permission_from_preflight(granted: bool) -> PermissionState {
    resolve_screen_permission(granted, EmpiricalCapture::Unknown)
}

/// 权限缺失时的错误，带可执行建议（打开系统设置深链）。
pub fn permission_denied_error() -> AppError {
    AppError::new(
        ErrorCode::CapturePermissionDenied,
        "缺少屏幕录制权限（CGPreflightScreenCaptureAccess 返回 false）",
    )
    .with_context("platform", "macos")
}

/// 打开系统设置的屏幕录制面板（macOS `open` 深链）。
pub fn open_system_settings() -> Result<(), AppError> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn()
        .map_err(|error| {
            AppError::new(
                ErrorCode::CapturePermissionDenied,
                format!("无法打开系统设置：{error}"),
            )
        })?;
    Ok(())
}

struct EmpiricalCache {
    checked_at: Instant,
    result: EmpiricalCapture,
}

static EMPIRICAL_CACHE: Mutex<Option<EmpiricalCache>> = Mutex::new(None);

fn invalidate_empirical_cache() {
    if let Ok(mut slot) = EMPIRICAL_CACHE.lock() {
        *slot = None;
    }
}

fn empirical_capture_cached() -> EmpiricalCapture {
    if let Ok(slot) = EMPIRICAL_CACHE.lock() {
        if let Some(cache) = slot.as_ref() {
            let ttl = Duration::from_secs(empirical_cache_ttl_secs(cache.result));
            if cache.checked_at.elapsed() < ttl {
                return cache.result;
            }
        }
    }
    let result = empirical_capture_with_timeout(EMPIRICAL_CAPTURE_TIMEOUT);
    if let Ok(mut slot) = EMPIRICAL_CACHE.lock() {
        *slot = Some(EmpiricalCache {
            checked_at: Instant::now(),
            result,
        });
    }
    result
}

fn empirical_capture_with_timeout(timeout: Duration) -> EmpiricalCapture {
    empirical_from_worker_timeout(timeout, empirical_capture_blocking)
}

/// 采一帧主屏并做黑帧判定。无显示器或枚举失败 → Unknown（不把无头环境误判成 Denied）。
fn empirical_capture_blocking() -> EmpiricalCapture {
    let Ok(monitors) = xcap::Monitor::all() else {
        return EmpiricalCapture::Unknown;
    };
    let Some(monitor) = monitors.first() else {
        return EmpiricalCapture::Unknown;
    };
    let Ok(frame) = monitor.capture_image() else {
        return EmpiricalCapture::Denied;
    };
    let Ok(rgb) = rgba_to_rgb(frame.width(), frame.height(), frame.as_raw()) else {
        return EmpiricalCapture::Denied;
    };
    let Ok(detector) = DHashDetector::new(HashPolicy::default()) else {
        return EmpiricalCapture::Unknown;
    };
    let stats = detector.analyze(&rgb_to_luma(&rgb));
    if detector.is_black_frame(&stats) {
        EmpiricalCapture::Denied
    } else {
        EmpiricalCapture::Works
    }
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
