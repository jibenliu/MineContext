//! macOS 窗口采集：属性映射必须准确。
//!
//! 这些断言是**纯函数**层面的（xcap 的属性 → 采集目标），因此不需要屏幕录制
//! 权限也能跑。真机取窗口列表/抓窗口图像在 `macos_source.rs` 的 `#[ignore]`
//! 测试里，未授权时自行跳过。
//!
//! 为什么窗口采集是隐私的关键：`blocked_apps` / `blocked_window_patterns`
//! 判定要靠**应用名与窗口标题**，而只截显示器的源这两项永远是空的 ——
//! 规则写了也不会命中。所以「窗口目标带着应用名与标题」本身就是一条隐私能力。

#![cfg(target_os = "macos")]

use mc_capture::platform::macos::window::window_target;
use mc_capture::source::{TargetBounds, TargetKind};

#[test]
fn window_target_carries_app_and_title_for_privacy_rules() {
    let target = window_target(
        42,
        Some("Figma".to_string()),
        Some("设计稿 - Figma".to_string()),
        Some(TargetBounds {
            x: 100,
            y: 50,
            width: 1440,
            height: 900,
        }),
        false,
        2.0,
    );

    assert_eq!(target.id, "window-42");
    assert_eq!(target.kind, TargetKind::Window);
    assert_eq!(target.app_name.as_deref(), Some("Figma"));
    assert_eq!(target.window_title.as_deref(), Some("设计稿 - Figma"));
    assert_eq!(target.scale_factor, 2.0);
    assert!(target.is_visible);
    assert_eq!(
        target.bounds,
        Some(TargetBounds {
            x: 100,
            y: 50,
            width: 1440,
            height: 900
        })
    );
}

#[test]
fn minimized_window_is_not_visible() {
    // 最小化的窗口没有可截内容；标记为不可见，调度器就不会去截它，
    // 也不会把「同一应用的旧标题」当成当前活动。
    let target = window_target(7, Some("Safari".to_string()), None, None, true, 1.0);

    assert!(!target.is_visible);
    assert_eq!(target.id, "window-7");
}

#[test]
fn missing_title_falls_back_to_app_name() {
    let target = window_target(9, Some("iTerm2".to_string()), None, None, false, 1.0);
    assert_eq!(target.name, "iTerm2");

    // 应用名与标题都拿不到时也要有个人能认出来的名字（而不是空字符串）
    let unnamed = window_target(11, None, None, None, false, 1.0);
    assert_eq!(unnamed.name, "window-11");
    assert_eq!(unnamed.app_name, None);
    assert_eq!(unnamed.window_title, None);
}

#[tokio::test]
#[ignore = "需要屏幕录制权限；用 --ignored 手工运行"]
async fn real_window_capture_reports_app_and_title() {
    use mc_capture::source::{CaptureContext, CaptureSource};
    use mc_common::time::Timestamp;

    let source = mc_capture::platform::macos::window::MacWindowSource::new().expect("构造窗口源");

    let health = source.health().await;
    if health.permission != mc_capture::source::PermissionState::Granted {
        // 未授权时**跳过并通过**（与 macos_source.rs 的像素测试同一约定）：
        // 这是「未验证」，由 scripts/verify-external.sh 识别并记为 SKIP。
        eprintln!("跳过：本机未授予屏幕录制权限");
        return;
    }

    let targets = source.enumerate().await.expect("枚举窗口");
    assert!(
        targets
            .iter()
            .any(|t| t.app_name.is_some() || t.window_title.is_some()),
        "至少要有一个窗口带着应用名或标题 —— 否则隐私黑名单永远不会命中"
    );

    let captures = source
        .poll(&CaptureContext::at(Timestamp::from_millis(
            mc_testkit::fixtures::FIXTURE_EPOCH_MS,
        )))
        .await
        .expect("抓取窗口元数据");

    // 只采前台窗口，且只采元数据（像素由屏幕源负责）
    assert!(
        captures.len() <= 1,
        "至多采一个前台窗口：{}",
        captures.len()
    );
    for capture in &captures {
        assert_eq!(capture.source_id, "macos:window");
        assert!(capture.image.is_none(), "窗口源不产图像");
        assert!(
            capture.target.app_name.is_some() || capture.target.window_title.is_some(),
            "窗口观测必须带应用名或标题，否则隐私规则无从判定"
        );
    }
}
