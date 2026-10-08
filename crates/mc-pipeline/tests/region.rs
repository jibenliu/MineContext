//! 区域采集（`screenshot_region`）。
//!
//! 语义：
//! 设了 `screenshot_region`（`[left, top, right, bottom]`，虚拟屏坐标）就
//! **只截这一块**；否则 `sct.monitors[1:]` 逐个显示器各截一张。
//!
//! 我们的实现放在采集环这一层（而不是平台层）：
//! 先按显示器采集，再裁到区域。这样 Windows/Linux 与 macOS 共用同一套逻辑，
//! 而且裁剪的边界条件（越界、跨屏、空区域）可以穷举测试。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use image::{Rgb, RgbImage};
use mc_capture::change::HashPolicy;
use mc_capture::scheduler::{CapturePolicy, CaptureSignals};
use mc_capture::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind, TargetBounds,
};
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_pipeline::pump::{
    CapturePump, CapturedObservation, FrameStore, ObservationSink, PumpPolicy, RegionRect,
    StoredFrame,
};
use mc_testkit::TestClock;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

/// 双屏：`display-1` 在 (0,0) 1600×1000，`display-2` 在 (1600,0) 1920×1080。
struct BoundedSource {
    /// 每帧用不同内容，避免被变化检测判成「没变」
    tick: AtomicUsize,
}

impl BoundedSource {
    fn new() -> Self {
        Self {
            tick: AtomicUsize::new(0),
        }
    }

    fn targets(&self) -> Vec<CaptureTarget> {
        let mut first = CaptureTarget::screen("display-1", "Display 1", 1.0);
        first.bounds = Some(TargetBounds {
            x: 0,
            y: 0,
            width: 1600,
            height: 1000,
        });

        let mut second = CaptureTarget::screen("display-2", "Display 2", 1.0);
        second.bounds = Some(TargetBounds {
            x: 1600,
            y: 0,
            width: 1920,
            height: 1080,
        });

        vec![first, second]
    }
}

#[async_trait]
impl CaptureSource for BoundedSource {
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
        Ok(self.targets())
    }
    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        let tick = self.tick.fetch_add(1, Ordering::SeqCst) as u8;
        Ok(ctx
            .targets
            .iter()
            .filter_map(|id| {
                let target = self.targets().into_iter().find(|target| &target.id == id)?;
                let bounds = target.bounds?;
                // 帧尺寸 = 显示器尺寸；像素值带上 tick 以便产生「变化」
                let image = RgbImage::from_fn(bounds.width, bounds.height, |x, y| {
                    Rgb([(x % 251) as u8, (y % 241) as u8, tick])
                });
                Some(RawCapture {
                    source_id: "fake:screen".to_string(),
                    source_kind: SourceKind::Screen,
                    target,
                    captured_at: ctx.now,
                    image: Some(image),
                    text: None,
                })
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
    sizes: Mutex<Vec<(u32, u32)>>,
}

impl FrameStore for MemoryFrames {
    fn store_frame(&self, image: &RgbImage, _at: Timestamp) -> Result<StoredFrame, AppError> {
        self.sizes
            .lock()
            .unwrap()
            .push((image.width(), image.height()));
        Ok(StoredFrame {
            relative_path: format!("screenshots/2026/09/30/{}.png", image.width()),
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

fn policy(region: Option<RegionRect>) -> PumpPolicy {
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
        selected_targets: Vec::new(),
        region,
    }
}

async fn tick_twice(pump: &mut CapturePump) {
    let clock = TestClock::from_millis(T0);
    for _ in 0..2 {
        clock.advance(std::time::Duration::from_secs(2));
        use mc_common::time::Clock as _;
        let _ = pump.tick(clock.now(), &CaptureSignals::RUNNING).await;
    }
}

// ---------------------------------------------------------------- 区域裁剪

#[tokio::test]
async fn region_crops_the_frame_to_the_requested_rect() {
    let sink = Arc::new(RecordingSink::default());
    let frames = Arc::new(MemoryFrames::default());
    let mut pump = CapturePump::new(
        Arc::new(BoundedSource::new()) as Arc<dyn CaptureSource>,
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        // 左屏中间一块 400×300
        policy(Some(RegionRect::new(100, 200, 500, 500).expect("合法区域"))),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    let sizes = frames.sizes.lock().unwrap().clone();
    assert!(
        sizes.contains(&(400, 300)),
        "落盘尺寸应当等于区域尺寸（400×300），实际 {sizes:?}"
    );
}

#[tokio::test]
async fn region_on_the_second_display_uses_that_display_only() {
    let sink = Arc::new(RecordingSink::default());
    let frames = Arc::new(MemoryFrames::default());
    let mut pump = CapturePump::new(
        Arc::new(BoundedSource::new()) as Arc<dyn CaptureSource>,
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        // 右屏（x 从 1600 开始）里的一块 640×480
        policy(Some(
            RegionRect::new(1700, 100, 2340, 580).expect("合法区域"),
        )),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    let observed: Vec<String> = sink
        .observations
        .lock()
        .unwrap()
        .iter()
        .map(|observation| observation.target.id.clone())
        .collect();
    assert!(
        observed.iter().all(|id| id == "display-2"),
        "区域落在右屏时只该采集右屏：{observed:?}"
    );

    let sizes = frames.sizes.lock().unwrap().clone();
    assert!(sizes.contains(&(640, 480)), "实际 {sizes:?}");
}

/// 区域超出显示器边界时按边界裁剪（而不是丢弃或报错）
#[tokio::test]
async fn region_is_clamped_to_the_display_bounds() {
    let sink = Arc::new(RecordingSink::default());
    let frames = Arc::new(MemoryFrames::default());
    let mut pump = CapturePump::new(
        Arc::new(BoundedSource::new()) as Arc<dyn CaptureSource>,
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        // 右下角越界：左屏只有 1600×1000
        policy(Some(
            RegionRect::new(1500, 900, 2500, 1500).expect("合法区域"),
        )),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    let sizes = frames.sizes.lock().unwrap().clone();
    assert_eq!(
        sizes.first().copied(),
        Some((100, 100)),
        "应当裁到显示器边界内（1600-1500 × 1000-900）：{sizes:?}"
    );
}

/// 区域完全在所有显示器之外：什么都不采（不是「退回全屏采集」）
#[tokio::test]
async fn region_outside_all_displays_captures_nothing() {
    let sink = Arc::new(RecordingSink::default());
    let frames = Arc::new(MemoryFrames::default());
    let mut pump = CapturePump::new(
        Arc::new(BoundedSource::new()) as Arc<dyn CaptureSource>,
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(Some(
            RegionRect::new(9000, 9000, 9100, 9100).expect("合法区域"),
        )),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    assert!(sink.observations.lock().unwrap().is_empty());
    assert!(frames.sizes.lock().unwrap().is_empty());
}

/// 源不报告几何信息时退回「第一个可见目标整屏采集」——
/// 降级是明确的，而不是静默什么都不采。
#[tokio::test]
async fn source_without_bounds_falls_back_to_the_first_target() {
    struct NoBoundsSource;

    #[async_trait]
    impl CaptureSource for NoBoundsSource {
        fn id(&self) -> &str {
            "fake:nobounds"
        }
        fn kind(&self) -> SourceKind {
            SourceKind::Screen
        }
        fn capabilities(&self) -> SourceCapabilities {
            SourceCapabilities::SCREEN
        }
        async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
            Ok(vec![CaptureTarget::screen("display-1", "Display 1", 1.0)])
        }
        async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
            Ok(ctx
                .targets
                .iter()
                .map(|id| RawCapture {
                    source_id: "fake:nobounds".to_string(),
                    source_kind: SourceKind::Screen,
                    target: CaptureTarget::screen(id.clone(), id.clone(), 1.0),
                    captured_at: ctx.now,
                    image: Some(RgbImage::from_pixel(
                        64,
                        48,
                        Rgb([(ctx.now.as_millis() % 251) as u8, 0, 0]),
                    )),
                    text: None,
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

    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::new(NoBoundsSource) as Arc<dyn CaptureSource>,
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(Some(RegionRect::new(10, 10, 50, 50).expect("合法区域"))),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    assert_eq!(
        sink.observations.lock().unwrap().len(),
        1,
        "没有几何信息时应当照常采集（区域无法应用，但不能什么都不采）"
    );
}

// ---------------------------------------------------------------- 回归

/// 不设区域时行为不变：逐个显示器各截一张全尺寸图
#[tokio::test]
async fn without_region_capture_stays_per_display() {
    let sink = Arc::new(RecordingSink::default());
    let frames = Arc::new(MemoryFrames::default());
    let mut pump = CapturePump::new(
        Arc::new(BoundedSource::new()) as Arc<dyn CaptureSource>,
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(None),
    )
    .unwrap();

    tick_twice(&mut pump).await;

    let mut sizes = frames.sizes.lock().unwrap().clone();
    sizes.sort();
    assert_eq!(
        sizes,
        vec![(1600, 1000), (1920, 1080)],
        "两台显示器各一张全尺寸图"
    );
}
