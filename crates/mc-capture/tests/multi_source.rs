//! `capture.sources` 必须真的决定「跑哪些采集源」。
//!
//! 配置里写着 `sources = ["screen", "window"]`，就必须真的构造两个源 ——
//! 这一组测试把「多源」钉成事实：
//! 合并目标、逐源轮询、**每条观测带来源**、单源失败不拖垮其它源。

use std::sync::Arc;

use mc_capture::composite::CompositeSource;
use mc_capture::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind, TargetKind, TextEvidence, TextOrigin,
};
use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_testkit::capture::FakeCaptureSource;

fn at(offset_ms: i64) -> Timestamp {
    Timestamp::from_millis(mc_testkit::fixtures::FIXTURE_EPOCH_MS + offset_ms)
}

fn screen_fake() -> Arc<dyn CaptureSource> {
    Arc::new(
        FakeCaptureSource::builder()
            .screen("display-1", "内建显示器", 2.0)
            .build()
            .expect("fake screen 可构造"),
    )
}

fn window_fake() -> Arc<dyn CaptureSource> {
    Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Window)
            .window("win-42", "设计稿 - Figma", "Figma")
            .build()
            .expect("fake window 可构造"),
    )
}

#[tokio::test]
async fn enumerate_merges_targets_from_every_source() {
    let composite = CompositeSource::new(vec![screen_fake(), window_fake()]);

    let targets = composite.enumerate().await.expect("枚举不应失败");
    let ids: Vec<&str> = targets.iter().map(|t| t.id.as_str()).collect();

    assert!(
        ids.contains(&"display-1") && ids.contains(&"win-42"),
        "两个源的目标都要出现，实际 {ids:?}"
    );
    assert_eq!(composite.ids(), vec!["fake:screen", "fake:window"]);
}

#[tokio::test]
async fn duplicate_target_ids_are_collapsed() {
    // 同一块屏被两个源同时枚举：不能出现两次，否则同一帧会被采集两次。
    let first = Arc::new(
        FakeCaptureSource::builder()
            .screen("display-1", "内建显示器", 2.0)
            .build()
            .unwrap(),
    );
    let second = Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Window)
            .screen("display-1", "内建显示器", 2.0)
            .build()
            .unwrap(),
    );

    let composite = CompositeSource::new(vec![first, second]);
    let targets = composite.enumerate().await.expect("枚举不应失败");

    assert_eq!(targets.len(), 1, "重复目标应被折叠，实际 {targets:?}");
}

#[tokio::test]
async fn poll_stamps_each_capture_with_its_own_source() {
    let composite = CompositeSource::new(vec![screen_fake(), window_fake()]);

    let captures = composite
        .poll(&CaptureContext::at(at(0)))
        .await
        .expect("轮询不应失败");

    let screen = captures
        .iter()
        .find(|c| c.target.id == "display-1")
        .expect("屏幕观测");
    let window = captures
        .iter()
        .find(|c| c.target.id == "win-42")
        .expect("窗口观测");

    // 来源必须逐条保留：混成一个 "composite" 之后，
    //「这条观测是截图还是窗口」就查不出来了。
    assert_eq!(screen.source_id, "fake:screen");
    assert_eq!(screen.source_kind, SourceKind::Screen);
    assert_eq!(window.source_id, "fake:window");
    assert_eq!(window.source_kind, SourceKind::Window);
    assert_eq!(window.target.kind, TargetKind::Window);
    assert_eq!(window.target.app_name.as_deref(), Some("Figma"));
}

#[tokio::test]
async fn one_denied_source_does_not_stop_the_others() {
    let denied = Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Window)
            .window("win-42", "设计稿 - Figma", "Figma")
            .permission(PermissionState::Denied)
            .build()
            .unwrap(),
    );
    let composite = CompositeSource::new(vec![screen_fake(), denied]);

    let captures = composite
        .poll(&CaptureContext::at(at(0)))
        .await
        .expect("单源失败不应让整轮轮询失败");

    assert_eq!(captures.len(), 1, "屏幕观测仍然要产出");
    assert_eq!(captures[0].source_id, "fake:screen");
    assert!(
        composite.last_errors().contains_key("fake:window"),
        "失败来源要留下原因，实际 {:?}",
        composite.last_errors()
    );
}

#[tokio::test]
async fn all_sources_denied_surfaces_permission_error() {
    // 双源都缺屏幕录制权限时，不能 Ok([]) 把失败吞掉——否则采集环记 0 失败、
    // UI「0 张截图已采集」且没有任何权限提示，重启也看不出原因。
    let screen = Arc::new(
        FakeCaptureSource::builder()
            .screen("display-1", "内建显示器", 2.0)
            .permission(PermissionState::Denied)
            .build()
            .unwrap(),
    ) as Arc<dyn CaptureSource>;
    let window = Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Window)
            .window("win-1", "Safari", "Safari")
            .permission(PermissionState::Denied)
            .build()
            .unwrap(),
    ) as Arc<dyn CaptureSource>;
    let composite = CompositeSource::new(vec![screen, window]);

    let error = composite
        .poll(&CaptureContext::at(at(0)))
        .await
        .expect_err("全部源失败必须向上抛错");

    assert_eq!(error.code(), ErrorCode::CapturePermissionDenied);
    assert!(
        composite.last_errors().len() >= 2,
        "每个失败源都应留下原因，实际 {:?}",
        composite.last_errors()
    );
}

#[tokio::test]
async fn health_reports_degraded_when_a_source_is_unavailable() {
    let denied = Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Window)
            .window("win-42", "设计稿 - Figma", "Figma")
            .permission(PermissionState::Denied)
            .build()
            .unwrap(),
    );
    let composite = CompositeSource::new(vec![screen_fake(), denied]);

    let health = composite.health().await;

    // 屏幕还能用，所以整体可用 —— 但必须**说明**少了什么，
    // 否则用户只会看到「窗口采集莫名其妙没有数据」。
    assert!(health.available);
    let message = health.message.unwrap_or_default();
    assert!(
        message.contains("fake:window"),
        "要让用户看出是哪个源不可用，实际 {message}"
    );

    let all_denied = CompositeSource::new(vec![Arc::new(
        FakeCaptureSource::builder()
            .kind(SourceKind::Window)
            .window("win-42", "设计稿 - Figma", "Figma")
            .permission(PermissionState::Denied)
            .build()
            .unwrap(),
    )]);
    assert!(
        !all_denied.health().await.available,
        "全部源都不可用时应报不可用"
    );
}

// 并发轮询：一次 tick 的耗时应该是最慢的源，而不是所有源之和。
// 用「每个源各睡 200ms」来区分：串行 ≈ 400ms，并发 ≈ 200ms。
struct SlowSource {
    id: &'static str,
    delay_ms: u64,
    kind: SourceKind,
}

#[async_trait::async_trait]
impl CaptureSource for SlowSource {
    fn id(&self) -> &str {
        self.id
    }

    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            produces_image: true,
            produces_text: false,
            needs_permission_granted: false,
        }
    }

    async fn health(&self) -> SourceHealth {
        SourceHealth {
            available: true,
            permission: PermissionState::Granted,
            message: None,
        }
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, mc_common::error::AppError> {
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        Ok(vec![target(self.id, TargetKind::Screen)])
    }

    async fn poll(
        &self,
        ctx: &CaptureContext,
    ) -> Result<Vec<RawCapture>, mc_common::error::AppError> {
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        Ok(vec![RawCapture {
            source_id: self.id.to_string(),
            source_kind: self.kind,
            target: target(self.id, TargetKind::Screen),
            captured_at: ctx.now,
            image: None,
            text: Some(TextEvidence {
                text: format!("来自 {}", self.id),
                origin: TextOrigin::Clipboard,
                confidence: 1.0,
            }),
        }])
    }
}

fn target(id: &str, kind: TargetKind) -> CaptureTarget {
    CaptureTarget {
        id: format!("{id}-target"),
        name: id.to_string(),
        kind,
        scale_factor: 1.0,
        display_id: None,
        window_id: None,
        app_name: None,
        window_title: None,
        is_visible: true,
        bounds: None,
    }
}

#[tokio::test]
async fn slow_sources_are_polled_concurrently() {
    let composite = CompositeSource::new(vec![
        Arc::new(SlowSource {
            id: "slow-a",
            delay_ms: 200,
            kind: SourceKind::Screen,
        }) as Arc<dyn CaptureSource>,
        Arc::new(SlowSource {
            id: "slow-b",
            delay_ms: 200,
            kind: SourceKind::Window,
        }),
    ]);

    let ctx = CaptureContext::at(at(0));
    let started = std::time::Instant::now();
    let captures = composite.poll(&ctx).await.expect("轮询应当成功");
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(350),
        "两个各睡 200ms 的源必须并发执行（串行要 400ms+），实际 {elapsed:?}"
    );
    assert_eq!(
        captures
            .iter()
            .map(|c| c.source_id.as_str())
            .collect::<Vec<_>>(),
        vec!["slow-a", "slow-b"],
        "并发轮询的结果仍按源的顺序合并"
    );
}
