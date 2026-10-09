//! 端到端采集环。
//!
//! 把调度器、采集源、变化检测、blob 存储、观测落库用**假实现**串起来验证。
//! 这是测试金字塔的集成层：不需要屏幕权限、不需要真实显示器、不需要网络，
//! 但它回答了一个单元测试回答不了的问题：**这些部件拼在一起到底对不对**。
//!
//! 核心不变量（并发与背压的取舍见 `docs/architecture.md`）：
//! **过载时降频，但绝不丢观测。**

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::RgbImage;
use mc_capture::change::HashPolicy;
use mc_capture::scheduler::CapturePolicy;
use mc_capture::source::{SourceKind, TargetKind};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, Timestamp};
use mc_pipeline::pump::{
    CapturePump, CapturedObservation, FrameStore, ObservationSink, PrivacyVerdict, PumpPolicy,
    StoredFrame,
};
use mc_testkit::capture::FakeCaptureSource;
use mc_testkit::TestClock;

// ---------------------------------------------------------------- 测试替身

#[derive(Default)]
struct InMemoryFrameStore {
    frames: Mutex<Vec<StoredFrame>>,
    calls: AtomicUsize,
}

impl FrameStore for InMemoryFrameStore {
    fn store_frame(&self, image: &RgbImage, _at: Timestamp) -> Result<StoredFrame, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let bytes = image.as_raw().len() as u64;
        let hash = blake3::hash(image.as_raw()).to_hex().to_string();
        let frame = StoredFrame {
            relative_path: format!("screenshots/2026/09/30/{hash}.png"),
            content_hash: hash,
            thumbnail_path: Some("thumbnails/2026/09/30/t.png".to_string()),
            width: image.width(),
            height: image.height(),
            bytes,
        };
        self.frames.lock().unwrap().push(frame.clone());
        Ok(frame)
    }
}

#[derive(Default)]
struct RecordingSink {
    observations: Mutex<Vec<CapturedObservation>>,
    fail_times: AtomicUsize,
    calls: AtomicUsize,
}

impl RecordingSink {
    fn failing(times: usize) -> Self {
        Self {
            fail_times: AtomicUsize::new(times),
            ..Default::default()
        }
    }

    fn count(&self) -> usize {
        self.observations.lock().unwrap().len()
    }
}

impl ObservationSink for RecordingSink {
    fn persist(&self, observation: &CapturedObservation) -> Result<(), AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let remaining = self.fail_times.load(Ordering::SeqCst);
        if remaining > 0 {
            self.fail_times.fetch_sub(1, Ordering::SeqCst);
            return Err(AppError::new(
                ErrorCode::StorageUnavailable,
                "模拟写入失败（数据库忙）",
            ));
        }
        self.observations.lock().unwrap().push(observation.clone());
        Ok(())
    }
}

fn screen(name: &str) -> FakeCaptureSource {
    FakeCaptureSource::builder()
        .screen(name, name, 2.0)
        .image_size(320, 200)
        .build()
        .unwrap()
}

struct Harness {
    pump: CapturePump,
    frames: Arc<InMemoryFrameStore>,
    sink: Arc<RecordingSink>,
    clock: TestClock,
}

fn harness(source: FakeCaptureSource, policy: PumpPolicy) -> Harness {
    let frames = Arc::new(InMemoryFrameStore::default());
    let sink = Arc::new(RecordingSink::default());
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    let pump = CapturePump::new(
        Arc::new(source),
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy,
    )
    .expect("pump 必须可构造");

    Harness {
        pump,
        frames,
        sink,
        clock,
    }
}

fn default_policy() -> PumpPolicy {
    PumpPolicy {
        capture: CapturePolicy {
            interval_secs: 1,
            idle_interval_secs: 10,
            idle_threshold_secs: 5,
            queue_capacity: 8,
            max_parallel_targets: 4,
        },
        hash: HashPolicy::default(),
        blocked_apps: Vec::new(),
        blocked_window_patterns: Vec::new(),
        blocked_domains: mc_pipeline::domains::DomainRules::default(),
        selected_targets: Vec::new(),
        region: None,
    }
}

// ---------------------------------------------------------------- 正常路径

#[tokio::test]
async fn first_tick_captures_stores_and_persists() {
    let mut h = harness(screen("display-1"), default_policy());

    let outcome = h
        .pump
        .tick(
            h.clock.now(),
            &mc_capture::scheduler::CaptureSignals::RUNNING,
        )
        .await
        .unwrap();

    assert_eq!(outcome.started, vec!["display-1"]);
    assert_eq!(h.frames.calls.load(Ordering::SeqCst), 1, "应当存了一次图");
    assert_eq!(h.sink.count(), 1, "应当落了一条观测");

    let obs = h.sink.observations.lock().unwrap()[0].clone();
    assert_eq!(obs.target.id, "display-1");
    assert!(obs.image.is_some(), "有意义的变化必须带图像");
    assert_eq!(obs.privacy_verdict, PrivacyVerdict::Allowed);
    assert!(!obs.idempotency.is_empty());
}

#[tokio::test]
async fn unchanged_frames_are_not_stored_nor_persisted() {
    let mut h = harness(screen("display-1"), default_policy());
    let running = mc_capture::scheduler::CaptureSignals::RUNNING;

    h.pump.tick(h.clock.now(), &running).await.unwrap();
    assert_eq!(h.sink.count(), 1);

    // 画面完全没变（FakeCaptureSource 每帧内容相同）
    h.clock.advance(std::time::Duration::from_secs(1));
    let second = h.pump.tick(h.clock.now(), &running).await.unwrap();
    assert_eq!(
        second.started,
        vec!["display-1"],
        "仍然会抓帧（要判断有没有变化）"
    );

    assert_eq!(
        h.sink.count(),
        1,
        "画面没变的帧不应再落一条观测 —— 这是「不花冤枉钱」的关键"
    );
    assert_eq!(
        h.frames.calls.load(Ordering::SeqCst),
        1,
        "画面没变就不该写图片（省磁盘）"
    );
    assert_eq!(h.pump.stats().unchanged, 1);
}

#[tokio::test]
async fn skipped_observations_are_counted_not_silent() {
    let mut h = harness(screen("display-1"), default_policy());
    let running = mc_capture::scheduler::CaptureSignals::RUNNING;

    for _ in 0..4 {
        h.pump.tick(h.clock.now(), &running).await.unwrap();
        h.clock.advance(std::time::Duration::from_secs(1));
    }

    let stats = h.pump.stats();
    assert_eq!(stats.persisted, 1, "只有第一帧是有意义的变化");
    assert_eq!(stats.unchanged, 3, "其余三帧应被记为「没变化」");
}

#[tokio::test]
async fn black_frames_are_capture_failures_not_persisted() {
    // macOS 无屏幕录制权限时常静默返回全黑帧；若当正常画面落盘/记 unchanged，
    // 用户会看到「0 张有内容」或假进度。必须记 failed 且不写盘。
    let source = FakeCaptureSource::builder()
        .screen("display-1", "Display 1", 2.0)
        .image_size(320, 200)
        .solid_rgb([0, 0, 0])
        .build()
        .unwrap();
    let mut h = harness(source, default_policy());

    h.pump
        .tick(
            h.clock.now(),
            &mc_capture::scheduler::CaptureSignals::RUNNING,
        )
        .await
        .unwrap();

    assert_eq!(h.frames.calls.load(Ordering::SeqCst), 0, "黑帧不得落盘");
    assert_eq!(h.sink.count(), 0, "黑帧不得落库");
    assert_eq!(h.pump.stats().failed, 1, "黑帧应计为采集失败");
    let err = h.pump.last_error().expect("应留下最近错误");
    assert_eq!(err.code(), ErrorCode::CaptureBlackFrame);
}

// ---------------------------------------------------------------- 隐私

#[tokio::test]
async fn privacy_blocked_app_is_not_stored_and_marked() {
    let mut policy = default_policy();
    policy.blocked_apps = vec!["1Password".to_string()];

    let source = FakeCaptureSource::builder()
        .window("win-1", "Vault — 1Password", "1Password")
        .image_size(320, 200)
        .build()
        .unwrap();

    let mut h = harness(source, policy);
    h.pump
        .tick(
            h.clock.now(),
            &mc_capture::scheduler::CaptureSignals::RUNNING,
        )
        .await
        .unwrap();

    assert_eq!(
        h.frames.calls.load(Ordering::SeqCst),
        0,
        "被隐私规则拦截的内容绝不允许落盘"
    );

    let obs = h.sink.observations.lock().unwrap()[0].clone();
    assert_eq!(obs.privacy_verdict, PrivacyVerdict::Blocked);
    assert!(obs.image.is_none(), "拦截的观测不得带图像引用");
    assert!(
        obs.target.window_title.is_none() && obs.target.app_name.is_some(),
        "审计信息只保留应用名，不保留可能含敏感内容的窗口标题"
    );
    assert_eq!(h.pump.stats().privacy_blocked, 1);
}

#[tokio::test]
async fn privacy_window_pattern_matches_case_insensitively() {
    let mut policy = default_policy();
    policy.blocked_window_patterns = vec!["password".to_string()];

    let source = FakeCaptureSource::builder()
        .window("win-1", "Enter Your PASSWORD", "Safari")
        .image_size(320, 200)
        .build()
        .unwrap();

    let mut h = harness(source, policy);
    h.pump
        .tick(
            h.clock.now(),
            &mc_capture::scheduler::CaptureSignals::RUNNING,
        )
        .await
        .unwrap();

    assert_eq!(h.frames.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        h.sink.observations.lock().unwrap()[0].privacy_verdict,
        PrivacyVerdict::Blocked
    );
}

#[tokio::test]
async fn non_blocked_window_is_allowed() {
    let mut policy = default_policy();
    policy.blocked_apps = vec!["1Password".to_string()];

    let source = FakeCaptureSource::builder()
        .window("win-1", "main.rs — VSCode", "Visual Studio Code")
        .image_size(320, 200)
        .build()
        .unwrap();

    let mut h = harness(source, policy);
    h.pump
        .tick(
            h.clock.now(),
            &mc_capture::scheduler::CaptureSignals::RUNNING,
        )
        .await
        .unwrap();

    assert_eq!(
        h.sink.observations.lock().unwrap()[0].privacy_verdict,
        PrivacyVerdict::Allowed
    );
    assert_eq!(h.frames.calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------- 锁屏 / 背压

#[tokio::test]
async fn locked_screen_captures_nothing() {
    let mut h = harness(screen("display-1"), default_policy());
    let locked = mc_capture::scheduler::CaptureSignals {
        locked: true,
        suspended: false,
        idle_for_secs: 0,
    };

    let outcome = h.pump.tick(h.clock.now(), &locked).await.unwrap();

    assert!(outcome.started.is_empty());
    assert_eq!(h.sink.count(), 0);
    assert_eq!(h.frames.calls.load(Ordering::SeqCst), 0);
}

/// 核心不变量：**过载时降频，但绝不丢观测**。
#[tokio::test]
async fn overload_throttles_instead_of_dropping_observations() {
    let mut policy = default_policy();
    policy.capture.queue_capacity = 2;

    // 4 个屏，队列容量只有 2：第二轮起必然触发背压
    let source = FakeCaptureSource::builder()
        .screen("d1", "d1", 2.0)
        .screen("d2", "d2", 2.0)
        .screen("d3", "d3", 2.0)
        .screen("d4", "d4", 2.0)
        .image_size(320, 200)
        .build()
        .unwrap();

    let frames = Arc::new(InMemoryFrameStore::default());
    // sink 永久失败 → 观测留在待写队列 → 队列占满 → 触发背压
    let sink = Arc::new(RecordingSink::failing(usize::MAX));
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    let mut pump = CapturePump::new(
        Arc::new(source),
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy,
    )
    .unwrap();

    let running = mc_capture::scheduler::CaptureSignals::RUNNING;
    for _ in 0..10 {
        pump.tick(clock.now(), &running).await.unwrap();
        clock.advance(std::time::Duration::from_secs(1));
    }

    let stats = pump.stats();
    assert_eq!(stats.dropped, 0, "任何情况下都不允许丢弃观测；过载只能降频");
    assert!(stats.failed > 0, "写入失败必须被计数");
    assert!(
        stats.throttled > 0,
        "待写队列满后必须触发降频，实际 throttled={}",
        stats.throttled
    );
    assert!(
        pump.pending_count() <= 2,
        "待写条数不得超过队列容量，实际 {}",
        pump.pending_count()
    );
    assert_eq!(
        sink.count(),
        0,
        "sink 一直失败，因此不应有任何观测被写入 —— 但它们也没被丢掉"
    );
}

/// 写入失败后恢复：积压必须被补写，而不是永久卡住。
#[tokio::test]
async fn transient_sink_failure_is_retried_and_recovered() {
    let frames = Arc::new(InMemoryFrameStore::default());
    let sink = Arc::new(RecordingSink::failing(1));
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    let mut pump = CapturePump::new(
        Arc::new(screen("display-1")),
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        default_policy(),
    )
    .unwrap();

    let running = mc_capture::scheduler::CaptureSignals::RUNNING;

    // 第一次 tick：写入失败，观测留在待写队列
    pump.tick(clock.now(), &running).await.unwrap();
    assert_eq!(sink.count(), 0);
    assert_eq!(pump.pending_count(), 1, "失败的观测必须留在队列里等待重试");
    assert_eq!(pump.stats().dropped, 0);

    // 下一次 tick 先重试补写
    clock.advance(std::time::Duration::from_secs(1));
    pump.tick(clock.now(), &running).await.unwrap();

    assert_eq!(sink.count(), 1, "第二次尝试应当写入成功");
    assert_eq!(pump.pending_count(), 0, "补写成功后队列应清空");
}

// ---------------------------------------------------------------- 观测身份

#[tokio::test]
async fn observation_ids_are_unique_and_time_ordered() {
    let frames = Arc::new(InMemoryFrameStore::default());
    let sink = Arc::new(RecordingSink::default());
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    // changing 模式：每轮内容不同 → 每轮都有有意义的变化
    let source = FakeCaptureSource::builder()
        .screen("d1", "d1", 2.0)
        .image_size(320, 200)
        .changing()
        .build()
        .unwrap();

    let mut pump = CapturePump::new(
        Arc::new(source),
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        default_policy(),
    )
    .unwrap();

    let running = mc_capture::scheduler::CaptureSignals::RUNNING;
    for _ in 0..4 {
        pump.tick(clock.now(), &running).await.unwrap();
        clock.advance(std::time::Duration::from_secs(1));
    }

    let observations = sink.observations.lock().unwrap().clone();
    assert_eq!(observations.len(), 4, "每轮内容都变了，应当 4 条观测");

    let mut ids: Vec<&str> = observations.iter().map(|o| o.id.as_str()).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "观测 id 必须唯一");

    // UUIDv7 的高 48 位是毫秒时间戳。同一毫秒内低位随机，
    // **不能**断言严格有序；但时间戳前缀必须是非递减的。
    let timestamps: Vec<&str> = observations.iter().map(|o| &o.id[..12]).collect();
    for pair in timestamps.windows(2) {
        assert!(
            pair[0] <= pair[1],
            "UUIDv7 的时间戳前缀必须非递减：{} vs {}",
            pair[0],
            pair[1]
        );
    }
}

#[tokio::test]
async fn idempotency_key_is_stable_for_identical_content() {
    let frames = Arc::new(InMemoryFrameStore::default());
    let sink = Arc::new(RecordingSink::default());
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    let source = FakeCaptureSource::builder()
        .screen("d1", "d1", 2.0)
        .screen("d2", "d2", 2.0)
        .image_size(320, 200)
        .build()
        .unwrap();

    let mut pump = CapturePump::new(
        Arc::new(source),
        Arc::clone(&frames) as Arc<dyn FrameStore>,
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        default_policy(),
    )
    .unwrap();

    pump.tick(clock.now(), &mc_capture::scheduler::CaptureSignals::RUNNING)
        .await
        .unwrap();

    let observations = sink.observations.lock().unwrap().clone();
    assert_eq!(observations.len(), 2, "两个屏各一条");

    // 幂等键必须包含源 id —— 否则两个屏同一时刻会被判为同一份内容
    assert_ne!(
        observations[0].idempotency, observations[1].idempotency,
        "不同采集源的幂等键必须不同"
    );
    assert!(
        observations[0].idempotency.contains("d1"),
        "幂等键应包含源标识，实际 {}",
        observations[0].idempotency
    );
}

// ---------------------------------------------------------------- 多屏与元数据

#[tokio::test]
async fn multi_display_tick_persists_one_observation_per_target() {
    let source = FakeCaptureSource::builder()
        .screen("d1", "Built-in Retina", 2.0)
        .screen("d2", "DELL U2720Q", 1.0)
        .image_size(320, 200)
        .build()
        .unwrap();

    let mut h = harness(source, default_policy());
    h.pump
        .tick(
            h.clock.now(),
            &mc_capture::scheduler::CaptureSignals::RUNNING,
        )
        .await
        .unwrap();

    let observations = h.sink.observations.lock().unwrap().clone();
    assert_eq!(observations.len(), 2);

    let ids: Vec<&str> = observations.iter().map(|o| o.target.id.as_str()).collect();
    assert_eq!(ids, vec!["d1", "d2"]);

    let retry: Vec<f32> = observations.iter().map(|o| o.target.scale_factor).collect();
    assert_eq!(retry, vec![2.0, 1.0], "Retina 缩放倍率必须一路带到观测里");
}

#[tokio::test]
async fn observation_records_target_kind_and_source() {
    let mut h = harness(screen("display-1"), default_policy());
    h.pump
        .tick(
            h.clock.now(),
            &mc_capture::scheduler::CaptureSignals::RUNNING,
        )
        .await
        .unwrap();

    let obs = h.sink.observations.lock().unwrap()[0].clone();
    assert_eq!(obs.target.kind, TargetKind::Screen);
    assert_eq!(obs.source_kind, SourceKind::Screen);
    assert_eq!(obs.source_id, "fake:screen");
    assert_eq!(obs.change_kind, mc_capture::change::ChangeKind::New);
}
