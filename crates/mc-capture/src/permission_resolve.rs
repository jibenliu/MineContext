//! 屏幕录制权限判定的纯规则（与平台探测分离，便于在无 macOS/TCC 的 CI 上单测）。
//!
//! `CGPreflightScreenCaptureAccess` 只反映调用进程的 TCC。采集在 `mc-daemon` 里跑，
//! 系统设置勾选的常常是外壳 app：Preflight 可能对实际可采的画面仍返回 false。
//! 交叉验证（能采到非黑帧）用于消除这类假阴性。

use crate::source::PermissionState;

/// Preflight 为 false 时的交叉探测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmpiricalCapture {
    /// 未做交叉探测（或探测不可用，例如无显示器）
    Unknown,
    /// 能采到非黑帧 → 权限实际可用
    Works,
    /// 采到黑帧或采集失败 → 权限仍不可用
    Denied,
}

/// Preflight + 经验证的合成规则。
///
/// Preflight 为 true 时直接信任；为 false 时若经验证能采到画面，按已授权处理。
pub const fn resolve_screen_permission(
    preflight_granted: bool,
    empirical: EmpiricalCapture,
) -> PermissionState {
    if preflight_granted {
        return PermissionState::Granted;
    }
    match empirical {
        EmpiricalCapture::Works => PermissionState::Granted,
        EmpiricalCapture::Denied | EmpiricalCapture::Unknown => PermissionState::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empirical_works_overrides_false_preflight() {
        assert_eq!(
            resolve_screen_permission(false, EmpiricalCapture::Works),
            PermissionState::Granted,
            "系统设置已授权但 Preflight 仍 false 时，应以能采到画面为准"
        );
    }

    #[test]
    fn empirical_denied_keeps_false_preflight() {
        assert_eq!(
            resolve_screen_permission(false, EmpiricalCapture::Denied),
            PermissionState::Denied
        );
        assert_eq!(
            resolve_screen_permission(false, EmpiricalCapture::Unknown),
            PermissionState::Denied
        );
    }

    #[test]
    fn true_preflight_ignores_empirical() {
        assert_eq!(
            resolve_screen_permission(true, EmpiricalCapture::Denied),
            PermissionState::Granted
        );
    }

    #[test]
    fn false_preflight_without_empirical_is_denied() {
        assert_eq!(
            resolve_screen_permission(false, EmpiricalCapture::Unknown),
            PermissionState::Denied
        );
    }
}
