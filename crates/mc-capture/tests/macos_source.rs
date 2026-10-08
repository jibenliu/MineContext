//! macOS 真实采集源。
//!
//! 分两类：
//! - **默认运行**：不依赖权限的断言（健康检查不 panic、错误映射）
//! - **`#[ignore]`**：真正读像素的断言。需要屏幕录制权限，因此不进 CI；
//!   用 `cargo test -p mc-capture --test macos_source -- --ignored --nocapture` 手工跑。
//!
//! 为什么真机测试也值得写：真机环境下最大的风险正是
//! 「权限没拿到 → macOS 静默返回全黑帧 → 用户以为一切正常」。
//! 只有真跑一次才能确认黑帧检测真的拦得住。

#![cfg(target_os = "macos")]

use mc_capture::change::{rgb_to_luma, ChangeDetector, DHashDetector, HashPolicy};
use mc_capture::platform::macos::MacScreenSource;
use mc_capture::source::{CaptureContext, CaptureSource, PermissionState};
use mc_common::time::Timestamp;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS;

fn source() -> MacScreenSource {
    MacScreenSource::new().expect("macOS 采集源必须可构造")
}

fn ctx() -> CaptureContext {
    CaptureContext {
        targets: Vec::new(),
        now: Timestamp::from_millis(FIXTURE_EPOCH_MS),
        idle: false,
    }
}

// ---------------------------------------------------------------- 无权限也能跑

#[tokio::test]
async fn health_never_panics_and_reports_a_known_state() {
    let health = source().health().await;

    assert!(
        matches!(
            health.permission,
            PermissionState::Granted | PermissionState::Denied | PermissionState::Unknown
        ),
        "权限状态必须是已知取值"
    );

    // 不可用时必须解释原因 —— 否则用户看到的是「没有黑盒的黑盒」
    if !health.available {
        assert!(
            health.message.is_some(),
            "不可用时必须给出原因，实际: {health:?}"
        );
    }
}

#[tokio::test]
async fn enumerate_does_not_panic_without_permission() {
    // 枚举显示器通常不需要屏幕录制权限；即便失败也必须是 typed error
    match source().enumerate().await {
        Ok(targets) => {
            for target in targets {
                assert!(!target.id.is_empty(), "目标 id 必须非空");
                assert!(target.scale_factor > 0.0, "缩放倍率必须为正");
            }
        }
        Err(error) => {
            assert!(
                !error.detail().is_empty(),
                "失败必须带可诊断的 detail，实际 {error:?}"
            );
        }
    }
}

#[tokio::test]
async fn poll_without_permission_returns_a_typed_error_not_a_black_frame() {
    let source = source();
    if source.permission() == PermissionState::Granted {
        // 本机已授权，跳过这条断言（真机路径由 ignored 用例覆盖）
        return;
    }

    let error = source
        .poll(&ctx())
        .await
        .expect_err("无权限时必须报错，而不是返回黑帧");

    assert_eq!(
        error.code(),
        mc_common::error::ErrorCode::CapturePermissionDenied
    );
    assert!(
        error.remediation().is_some(),
        "必须给出「去哪里授权」的建议"
    );
}

// ---------------------------------------------------------------- 真机（手工）

#[tokio::test]
#[ignore = "需要屏幕录制权限；用 --ignored 手工运行"]
async fn real_capture_enumerates_monitors() {
    let targets = source().enumerate().await.expect("枚举必须成功");
    assert!(!targets.is_empty(), "本机应当至少有一个显示器");

    for target in &targets {
        println!(
            "monitor: id={} name={} scale={}",
            target.id, target.name, target.scale_factor
        );
    }
}

#[tokio::test]
#[ignore = "需要屏幕录制权限；用 --ignored 手工运行"]
async fn real_capture_produces_a_frame_with_sane_dimensions() {
    let source = source();
    if source.permission() != PermissionState::Granted {
        // 没有权限时**跳过而不是失败** —— 这不是代码缺陷，
        // 而是环境未授权。真实断言的守护在
        // `poll_without_permission_returns_a_typed_error_not_a_black_frame` 里。
        eprintln!(
            "跳过：本机未授予屏幕录制权限。\
             请在「系统设置 → 隐私与安全性 → 屏幕录制」中授权后重跑。"
        );
        return;
    }

    let captures = source.poll(&ctx()).await.expect("采集必须成功");
    assert!(!captures.is_empty(), "应当至少采到一帧");

    for capture in &captures {
        let image = capture.image.as_ref().expect("屏幕源必须产出图像");
        println!(
            "captured {}: {}x{} (scale {})",
            capture.target.name,
            image.width(),
            image.height(),
            capture.target.scale_factor
        );
        assert!(image.width() > 100 && image.height() > 100, "分辨率不合理");
    }
}

/// **最关键的一条真机断言**：采到的不是黑帧。
///
/// 权限没生效时 macOS 会返回全黑图且不报错 —— 这条测试就是拦它的。
#[tokio::test]
#[ignore = "需要屏幕录制权限；用 --ignored 手工运行"]
async fn real_capture_is_not_a_black_frame() {
    let source = source();
    if source.permission() != PermissionState::Granted {
        eprintln!("跳过：本机未授予屏幕录制权限");
        return;
    }

    let captures = source.poll(&ctx()).await.expect("采集必须成功");
    let detector = DHashDetector::new(HashPolicy::default()).unwrap();

    for capture in &captures {
        let image = capture.image.as_ref().expect("必须有图像");
        let stats = detector.analyze(&rgb_to_luma(image));

        println!(
            "{}: mean_luma={:.1} variance={:.1} phash={:#x}",
            capture.target.name, stats.mean_luma, stats.luma_variance, stats.phash
        );

        assert!(
            !detector.is_black_frame(&stats),
            "采到了全黑帧 —— 通常是屏幕录制权限未生效。mean_luma={:.1}",
            stats.mean_luma
        );
        assert!(
            stats.luma_variance > 1.0,
            "画面方差过小，可能是黑帧或纯色遮挡：{:.2}",
            stats.luma_variance
        );
    }
}

#[tokio::test]
#[ignore = "需要屏幕录制权限；用 --ignored 手工运行"]
async fn capture_latency_is_within_budget() {
    use std::time::Instant;

    let source = source();
    if source.permission() != PermissionState::Granted {
        eprintln!("跳过：本机未授予屏幕录制权限");
        return;
    }

    // 预热一次（首次调用包含显示器枚举与内存分配）
    let _ = source.poll(&ctx()).await;

    let mut samples = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let captures = source.poll(&ctx()).await.expect("采集必须成功");
        samples.push(start.elapsed());
        assert!(!captures.is_empty());
    }

    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95) / 100];

    println!("capture latency: p50={p50:?} p95={p95:?}");

    // 延迟目标：p50 < 100ms，p95 < 250ms。
    // 这是 Intel Mac + Retina 3072x1920 的实测基线。
    assert!(
        p50.as_millis() < 100,
        "p50 截图延迟应当 < 100ms，实际 {p50:?}"
    );
    assert!(
        p95.as_millis() < 250,
        "p95 截图延迟应当 < 250ms，实际 {p95:?}"
    );
}
