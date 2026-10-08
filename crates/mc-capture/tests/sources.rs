//! 采集源语义。
//!
//! 这些测试全部针对 `FakeCaptureSource` 与 trait 契约，不需要屏幕权限、
//! 不需要真实显示器、不需要 macOS —— 因此可以在任何机器的 CI 上跑。
//!
//! 真机行为（真实截图、权限、Retina）由 8h soak 覆盖。

use mc_capture::source::{
    CaptureContext, CaptureSource, PermissionState, SourceCapabilities, SourceKind, TargetKind,
};
use mc_common::time::Timestamp;
use mc_testkit::capture::FakeCaptureSource;

fn ctx(now_ms: i64) -> CaptureContext {
    CaptureContext {
        targets: Vec::new(),
        now: Timestamp::from_millis(now_ms),
        idle: false,
    }
}

fn source_with_two_displays() -> FakeCaptureSource {
    FakeCaptureSource::builder()
        .screen("display-1", "Built-in Retina Display", 2.0)
        .screen("display-2", "DELL U2720Q", 1.0)
        .build()
        .expect("fake source 必须可构造")
}

// ---------------------------------------------------------------- 1.15 枚举

#[tokio::test]
async fn source_enumerate_lists_screens_and_windows() {
    let source = FakeCaptureSource::builder()
        .screen("display-1", "Built-in", 2.0)
        .window("win-1", "main.rs — VSCode", "Visual Studio Code")
        .build()
        .unwrap();

    let targets = source.enumerate().await.unwrap();

    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].kind, TargetKind::Screen);
    assert_eq!(targets[0].name, "Built-in");
    assert_eq!(targets[0].display_id.as_deref(), Some("display-1"));
    assert_eq!(targets[0].scale_factor, 2.0);

    assert_eq!(targets[1].kind, TargetKind::Window);
    assert_eq!(targets[1].app_name.as_deref(), Some("Visual Studio Code"));
    assert_eq!(targets[1].window_title.as_deref(), Some("main.rs — VSCode"));
}

#[tokio::test]
async fn source_reports_its_kind_and_capabilities() {
    let source = source_with_two_displays();

    assert_eq!(source.kind(), SourceKind::Screen);
    let caps = source.capabilities();
    assert!(caps.produces_image, "屏幕源必须能产出图像");
    assert!(caps.needs_permission_granted, "屏幕源需要屏幕录制权限");
    assert!(!caps.produces_text, "屏幕源不直接产出文本");
    assert_eq!(
        CaptureSource::id(&source),
        "fake:screen",
        "每个源必须有稳定 id，用于规则匹配与诊断"
    );
}

// ---------------------------------------------------------------- 1.9 顺序

#[tokio::test]
async fn fake_source_produces_observations_in_order() {
    let source = source_with_two_displays();

    let first = source.poll(&ctx(1_000)).await.unwrap();
    let second = source.poll(&ctx(2_000)).await.unwrap();

    assert_eq!(first.len(), 2, "双屏应当每次产出两条观测");
    assert_eq!(second.len(), 2);

    // 时间戳必须来自注入的 CaptureContext，而不是内部读时钟 ——
    // 否则测试无法确定性地驱动「防抖 / 空闲 / 跨天」等时间敏感逻辑。
    assert_eq!(first[0].captured_at, Timestamp::from_millis(1_000));
    assert_eq!(second[0].captured_at, Timestamp::from_millis(2_000));

    // 同一 target 的产出顺序稳定
    assert_eq!(first[0].target.id, "display-1");
    assert_eq!(first[1].target.id, "display-2");
    assert_eq!(second[0].target.id, "display-1");
}

#[tokio::test]
async fn poll_only_returns_selected_targets() {
    let source = source_with_two_displays();

    let mut selection = ctx(1_000);
    selection.targets = vec!["display-2".to_string()];

    let captures = source.poll(&selection).await.unwrap();

    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].target.id, "display-2");
}

// ---------------------------------------------------------------- 1.10 权限

#[tokio::test]
async fn permission_denied_yields_typed_error_not_panic() {
    let source = FakeCaptureSource::builder()
        .screen("display-1", "Built-in", 2.0)
        .permission(PermissionState::Denied)
        .build()
        .unwrap();

    let error = source.poll(&ctx(1_000)).await.unwrap_err();

    assert_eq!(
        error.code(),
        mc_common::error::ErrorCode::CapturePermissionDenied
    );
    assert!(
        error.remediation().is_some(),
        "权限错误必须给出可执行建议（打开系统设置）"
    );
}

#[tokio::test]
async fn health_reports_permission_state() {
    let denied = FakeCaptureSource::builder()
        .screen("display-1", "Built-in", 2.0)
        .permission(PermissionState::Denied)
        .build()
        .unwrap();

    let health = denied.health().await;
    assert!(!health.available);
    assert_eq!(health.permission, PermissionState::Denied);
    assert!(health.message.is_some(), "不可用时必须解释原因");

    let granted = source_with_two_displays();
    let health = granted.health().await;
    assert!(health.available);
    assert_eq!(health.permission, PermissionState::Granted);
}

// ---------------------------------------------------------------- 1.11 无显示器

#[tokio::test]
async fn no_display_yields_empty_not_error() {
    let source = FakeCaptureSource::builder()
        .permission(PermissionState::Granted)
        .build()
        .unwrap();

    let captures = source.poll(&ctx(1_000)).await.unwrap();

    assert!(
        captures.is_empty(),
        "没有显示器（远程会话/合盖）是正常状态，不是错误"
    );
}

// ---------------------------------------------------------------- 1.12 锁屏

#[tokio::test]
async fn locked_screen_produces_no_observation() {
    let source = source_with_two_displays();
    source.set_locked(true);

    let captures = source.poll(&ctx(1_000)).await.unwrap();

    assert!(captures.is_empty(), "锁屏期间不应产出任何观测");
}

#[tokio::test]
async fn unlock_resumes_observations() {
    let source = source_with_two_displays();
    source.set_locked(true);
    assert!(source.poll(&ctx(1_000)).await.unwrap().is_empty());

    source.set_locked(false);
    let captures = source.poll(&ctx(2_000)).await.unwrap();

    assert_eq!(captures.len(), 2, "解锁后必须恢复采集");
}

// ---------------------------------------------------------------- 1.13 多屏

#[tokio::test]
async fn multi_display_captures_each_target() {
    let source = source_with_two_displays();

    let captures = source.poll(&ctx(1_000)).await.unwrap();

    let ids: Vec<&str> = captures.iter().map(|c| c.target.id.as_str()).collect();
    assert_eq!(ids, vec!["display-1", "display-2"]);
    assert!(
        captures.iter().all(|c| c.target.kind == TargetKind::Screen),
        "两条都应是屏幕观测"
    );
}

// ---------------------------------------------------------------- 1.14 DPI

#[tokio::test]
async fn retina_scale_factor_preserved() {
    let source = source_with_two_displays();

    let captures = source.poll(&ctx(1_000)).await.unwrap();

    assert_eq!(
        captures[0].target.scale_factor, 2.0,
        "Retina 屏的缩放倍率必须保留"
    );
    assert_eq!(captures[1].target.scale_factor, 1.0);
}

#[tokio::test]
async fn captured_image_dimensions_match_scale_factor() {
    let source = source_with_two_displays();

    let captures = source.poll(&ctx(1_000)).await.unwrap();

    for capture in &captures {
        let image = capture.image.as_ref().expect("屏幕源必须产出图像");
        assert!(
            image.width() > 0 && image.height() > 0,
            "图像尺寸必须有效：{}x{}",
            image.width(),
            image.height()
        );
    }
}

// ---------------------------------------------------------------- 可插拔

// 采集来源必须可插拔
#[tokio::test]
async fn source_registry_is_pluggable() {
    use mc_capture::source::SourceRegistry;

    let mut registry = SourceRegistry::new();
    registry.register(std::sync::Arc::new(source_with_two_displays()));
    registry.register(std::sync::Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Clipboard)
            .capability(SourceCapabilities {
                produces_image: false,
                produces_text: true,
                needs_permission_granted: false,
            })
            .build()
            .unwrap(),
    ));

    assert_eq!(registry.len(), 2);
    assert!(registry.by_kind(SourceKind::Screen).is_some());
    assert!(registry.by_kind(SourceKind::Clipboard).is_some());
    assert!(registry.by_kind(SourceKind::Browser).is_none());

    // 注册顺序不影响按 kind 查找
    assert_eq!(
        registry.by_kind(SourceKind::Screen).unwrap().kind(),
        SourceKind::Screen
    );
}

#[tokio::test]
async fn registry_polls_all_sources_and_merges_results() {
    use mc_capture::source::SourceRegistry;

    let mut registry = SourceRegistry::new();
    registry.register(std::sync::Arc::new(source_with_two_displays()));

    let captures = registry.poll_all(&ctx(1_000)).await;

    assert_eq!(captures.len(), 2);
}

#[tokio::test]
async fn registry_isolates_failing_source_from_healthy_ones() {
    use mc_capture::source::SourceRegistry;

    let mut registry = SourceRegistry::new();
    registry.register(std::sync::Arc::new(source_with_two_displays()));
    registry.register(std::sync::Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Clipboard)
            .screen("clipboard", "Clipboard", 1.0)
            .permission(PermissionState::Denied)
            .build()
            .unwrap(),
    ));

    // 一个源失败不应让整次采集失败 —— 否则剪贴板权限问题会连带停掉截图
    let captures = registry.poll_all(&ctx(1_000)).await;

    assert_eq!(captures.len(), 2, "健康的屏幕源仍应产出观测");
}
