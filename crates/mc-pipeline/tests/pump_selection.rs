//! 显示器子集选择：只采集用户选中的目标。
//!
//! 语义是「可见的 ∩ 用户选中的」。选择若只存在**内存**里，重启之后即为空，
//! 于是什么都不采集，直到用户再打开设置页 —— 因此选择落在配置里，并明确规定：
//!
//! - 选择为空 = 全部可见目标（缺省是可用的，而不是「静默停采」）；
//! - 选择里出现已不存在的 id = 什么都采不到（**不能**退化成「那就全采」）——
//!   用户明确排除了一块屏，退化成全采就是把他的排除指令当没看见。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use image::RgbImage;
use mc_capture::change::HashPolicy;
use mc_capture::scheduler::{CapturePolicy, CaptureSignals};
use mc_capture::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind,
};
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_pipeline::pump::{
    CapturePump, CapturedObservation, FrameStore, ObservationSink, PumpPolicy, StoredFrame,
};
use mc_testkit::TestClock;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

/// 可指定可见性的假采集源：选择逻辑依赖 `is_visible`。
struct VisibilitySource {
    targets: Vec<CaptureTarget>,
}

impl VisibilitySource {
    fn new() -> Self {
        let mut visible_window = CaptureTarget::window("window-1", "main.rs", "Visual Studio Code");
        visible_window.is_visible = false;

        Self {
            targets: vec![
                CaptureTarget::screen("display-1", "Display 1", 2.0),
                CaptureTarget::screen("display-2", "Display 2", 2.0),
                CaptureTarget::screen("display-3", "Display 3", 1.0),
                visible_window,
            ],
        }
    }
}

#[async_trait]
impl CaptureSource for VisibilitySource {
    fn id(&self) -> &str {
        "fake:screen"
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Screen
    }
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::SCREEN
    }
    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        Ok(self.targets.clone())
    }
    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        Ok(ctx
            .targets
            .iter()
            .map(|id| {
                let target = self
                    .targets
                    .iter()
                    .find(|target| &target.id == id)
                    .cloned()
                    .unwrap_or_else(|| CaptureTarget::screen(id.clone(), id.clone(), 1.0));
                RawCapture {
                    source_id: "fake:screen".to_string(),
                    source_kind: SourceKind::Screen,
                    target,
                    captured_at: ctx.now,
                    // 每帧给不同内容，避免被变化检测判成「没变」
                    image: Some(RgbImage::from_pixel(
                        32,
                        24,
                        image::Rgb([(id.len() * 37) as u8, (ctx.now.as_millis() % 251) as u8, 90]),
                    )),
                    text: None,
                }
            })
            .collect())
    }
    async fn health(&self) -> SourceHealth {
        SourceHealth {
            available: true,
            permission: PermissionState::Granted,
            message: None,
        }
    }
}

#[derive(Default)]
struct MemoryFrames {
    calls: AtomicUsize,
}

impl FrameStore for MemoryFrames {
    fn store_frame(&self, image: &RgbImage, _at: Timestamp) -> Result<StoredFrame, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(StoredFrame {
            relative_path: format!(
                "screenshots/2026/09/30/{}.png",
                self.calls.load(Ordering::SeqCst)
            ),
            content_hash: "hash".to_string(),
            thumbnail_path: None,
            width: image.width(),
            height: image.height(),
            bytes: image.as_raw().len() as u64,
        })
    }
}

#[derive(Default)]
struct RecordingSink {
    observations: Mutex<Vec<CapturedObservation>>,
}

impl ObservationSink for RecordingSink {
    fn persist(&self, observation: &CapturedObservation) -> Result<(), AppError> {
        self.observations.lock().unwrap().push(observation.clone());
        Ok(())
    }
}

fn policy(selected: &[&str]) -> PumpPolicy {
    PumpPolicy {
        capture: CapturePolicy {
            interval_secs: 1,
            idle_interval_secs: 10,
            idle_threshold_secs: 5,
            queue_capacity: 16,
            max_parallel_targets: 8,
        },
        hash: HashPolicy::default(),
        blocked_apps: Vec::new(),
        blocked_window_patterns: Vec::new(),
        blocked_domains: mc_pipeline::domains::DomainRules::default(),
        selected_targets: selected.iter().map(|id| (*id).to_string()).collect(),
        region: None,
    }
}

/// 跑两轮：第一轮建立"上一帧"，第二轮才会产出变化。
async fn tick_twice(pump: &mut CapturePump) {
    let clock = TestClock::from_millis(T0);
    for _ in 0..2 {
        clock.advance(std::time::Duration::from_secs(2));
        use mc_common::time::Clock as _;
        let _ = pump.tick(clock.now(), &CaptureSignals::RUNNING).await;
    }
}

fn captured_targets(sink: &RecordingSink) -> Vec<String> {
    let mut ids: Vec<String> = sink
        .observations
        .lock()
        .unwrap()
        .iter()
        .map(|observation| observation.target.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

// ---------------------------------------------------------------- 选择

#[tokio::test]
async fn empty_selection_captures_every_visible_target() {
    let source = Arc::new(VisibilitySource::new());
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&[]),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    assert_eq!(
        captured_targets(&sink),
        vec!["display-1", "display-2", "display-3"],
        "选择为空时应采集所有**可见**目标（不可见的窗口目标不算）"
    );
}

#[tokio::test]
async fn selection_limits_capture_to_the_chosen_displays() {
    let source = Arc::new(VisibilitySource::new());
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&["display-2"]),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    assert_eq!(
        captured_targets(&sink),
        vec!["display-2"],
        "只应采集选中的那块屏"
    );
}

#[tokio::test]
async fn multiple_displays_can_be_selected() {
    let source = Arc::new(VisibilitySource::new());
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&["display-1", "display-3"]),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    assert_eq!(captured_targets(&sink), vec!["display-1", "display-3"]);
}

/// 选中了一块已经不存在的屏：什么都别采，**不要**退化成「全采」。
#[tokio::test]
async fn selection_of_a_missing_target_captures_nothing() {
    let source = Arc::new(VisibilitySource::new());
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&["display-99"]),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    assert!(
        captured_targets(&sink).is_empty(),
        "已消失的目标不该让采集退化为「全采」（那等于无视用户的排除）"
    );
}

/// 选中的目标不可见时同样不采集（可见性是硬前提）
#[tokio::test]
async fn invisible_targets_are_never_captured_even_if_selected() {
    let source = Arc::new(VisibilitySource::new());
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&["window-1", "display-1"]),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    assert_eq!(captured_targets(&sink), vec!["display-1"]);
}

/// 选择改变（重建 pump）后立刻生效 —— 采集环就是靠重建 pump 应用新配置的
#[tokio::test]
async fn changing_the_selection_takes_effect_immediately() {
    let source = Arc::new(VisibilitySource::new());

    let first_sink = Arc::new(RecordingSink::default());
    let mut first = CapturePump::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&first_sink) as Arc<dyn ObservationSink>,
        policy(&["display-1"]),
    )
    .unwrap();
    tick_twice(&mut first).await;
    assert_eq!(captured_targets(&first_sink), vec!["display-1"]);

    // 用户改选 display-3 → 采集环用新策略重建 pump
    let second_sink = Arc::new(RecordingSink::default());
    let mut second = CapturePump::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&second_sink) as Arc<dyn ObservationSink>,
        policy(&["display-3"]),
    )
    .unwrap();
    tick_twice(&mut second).await;

    assert_eq!(captured_targets(&second_sink), vec!["display-3"]);
}
