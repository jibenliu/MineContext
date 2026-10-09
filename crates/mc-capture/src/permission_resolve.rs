//! 屏幕录制权限判定的纯规则（与平台探测分离，便于在无 macOS/TCC 的 CI 上单测）。
//!
//! `CGPreflightScreenCaptureAccess` 只反映调用进程的 TCC。采集在 `mc-daemon` 里跑，
//! 系统设置勾选的常常是外壳 app：Preflight 可能对实际可采的画面仍返回 false。
//! 交叉验证（能采到非黑帧）用于消除这类假阴性。

use std::time::Duration;

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
    /// 交叉截屏超时（macOS 13 上无权限时 `capture_image` 可能挂起）→ 按不可用处理，避免拖死 daemon
    TimedOut,
}

/// Preflight + 经验证的合成规则。
///
/// Preflight 为 true 时直接信任；为 false 时若经验证能采到画面，按已授权处理。
/// 超时与未知一律 fail-closed：宁可提示缺权限，也不能让同步截屏挂起整条采集环。
pub const fn resolve_screen_permission(
    preflight_granted: bool,
    empirical: EmpiricalCapture,
) -> PermissionState {
    if preflight_granted {
        return PermissionState::Granted;
    }
    match empirical {
        EmpiricalCapture::Works => PermissionState::Granted,
        EmpiricalCapture::Denied | EmpiricalCapture::Unknown | EmpiricalCapture::TimedOut => {
            PermissionState::Denied
        }
    }
}

/// 经验证结果的缓存 TTL：否决/超时缓存更久，避免每几秒再堵一次同步截屏。
pub const fn empirical_cache_ttl_secs(result: EmpiricalCapture) -> u64 {
    match result {
        EmpiricalCapture::Works => 30,
        EmpiricalCapture::Denied | EmpiricalCapture::TimedOut => 60,
        EmpiricalCapture::Unknown => 8,
    }
}

/// 在独立线程跑交叉探测，超时返回 [`EmpiricalCapture::TimedOut`]。
///
/// 平台实现把真正的 `capture_image` 放进 `worker`；本函数保证调用方不会无限等待。
pub fn empirical_from_worker_timeout<F>(timeout: Duration, worker: F) -> EmpiricalCapture
where
    F: FnOnce() -> EmpiricalCapture + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::Builder::new()
        .name("mc-empirical-capture".into())
        .spawn(move || {
            let _ = tx.send(worker());
        });
    if handle.is_err() {
        return EmpiricalCapture::Unknown;
    }
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => EmpiricalCapture::TimedOut,
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

    #[test]
    fn empirical_timeout_is_denied_fail_closed() {
        assert_eq!(
            resolve_screen_permission(false, EmpiricalCapture::TimedOut),
            PermissionState::Denied,
            "截屏交叉验证超时不得当成已授权，否则会继续堵在采集路径上"
        );
    }

    #[test]
    fn denied_empirical_caches_longer_than_unknown() {
        assert!(
            empirical_cache_ttl_secs(EmpiricalCapture::Denied)
                > empirical_cache_ttl_secs(EmpiricalCapture::Unknown)
        );
        assert!(
            empirical_cache_ttl_secs(EmpiricalCapture::TimedOut)
                >= empirical_cache_ttl_secs(EmpiricalCapture::Denied)
        );
    }

    #[test]
    fn empirical_worker_timeout_maps_to_timed_out() {
        let result = empirical_from_worker_timeout(Duration::from_millis(30), || {
            std::thread::sleep(Duration::from_secs(10));
            EmpiricalCapture::Works
        });
        assert_eq!(
            result,
            EmpiricalCapture::TimedOut,
            "挂起的交叉截屏必须在超时后返回 TimedOut，不能一直堵着调用方"
        );
        assert_eq!(
            resolve_screen_permission(false, result),
            PermissionState::Denied
        );
    }

    #[test]
    fn empirical_worker_completes_before_timeout() {
        let result =
            empirical_from_worker_timeout(Duration::from_secs(2), || EmpiricalCapture::Works);
        assert_eq!(result, EmpiricalCapture::Works);
    }
}
