//! 窗口元数据必须真正落成观测，且**前台被拉黑时同轮的屏幕帧也一起拦下**。
//!
//! 两件事在同一片里做，因为缺一个另一个就没意义：
//!
//! 1. 窗口源产出的是 `image = None` 的元数据观测。它在里会被
//!    「没有图像就不处理」直接丢掉 —— 于是应用名/标题永远到不了落库与
//!    活动识别，`blocked_apps` 也永远不会命中。
//! 2. 屏幕截图里**同样包含**那个被拉黑的应用。窗口观测被拦住了，
//!    像素却照存，等于黑名单形同虚设。所以同一轮里只要前台窗口命中黑名单，
//!    这一轮的屏幕帧也一起拦下（fail-closed，宁可少存也不多存）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::RgbImage;
use mc_capture::change::HashPolicy;
use mc_capture::composite::CompositeSource;
use mc_capture::scheduler::{CapturePolicy, CaptureSignals};
use mc_capture::source::{CaptureSource, SourceCapabilities, SourceKind};
use mc_common::error::AppError;
use mc_common::time::{Clock as _, Timestamp};
use mc_pipeline::pump::{
    CapturePump, CapturedObservation, FrameStore, ObservationSink, PumpPolicy, StoredFrame,
};
use mc_testkit::capture::FakeCaptureSource;
use mc_testkit::TestClock;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

#[derive(Default)]
struct MemoryFrames {
    calls: AtomicUsize,
}

impl FrameStore for MemoryFrames {
    fn store_frame(&self, image: &RgbImage, _at: Timestamp) -> Result<StoredFrame, AppError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(StoredFrame {
            relative_path: format!("screenshots/2026/09/30/{n}.png"),
            content_hash: format!("hash-{n}"),
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

impl RecordingSink {
    fn all(&self) -> Vec<CapturedObservation> {
        self.observations.lock().unwrap().clone()
    }

    fn for_target(&self, id: &str) -> Vec<CapturedObservation> {
        self.all()
            .into_iter()
            .filter(|o| o.target.id == id)
            .collect()
    }
}

impl ObservationSink for RecordingSink {
    fn persist(&self, observation: &CapturedObservation) -> Result<(), AppError> {
        self.observations.lock().unwrap().push(observation.clone());
        Ok(())
    }
}

/// 屏幕源（产图像）+ 窗口源（只产元数据），合成一个源 —— 与 daemon 的接法一致。
fn sources(app: &str, title: &str) -> Arc<dyn CaptureSource> {
    let screen = FakeCaptureSource::builder()
        .screen("display-1", "内建显示器", 2.0)
        .changing()
        .build()
        .expect("屏幕假源");

    // 只产元数据（`image = None`），与 `macos:window` 的能力声明一致
    let window = FakeCaptureSource::builder()
        .kind(SourceKind::Window)
        .capability(SourceCapabilities::WINDOW_METADATA)
        .window("window-1", title, app)
        .build()
        .expect("窗口假源");

    Arc::new(CompositeSource::new(vec![
        Arc::new(screen) as Arc<dyn CaptureSource>,
        Arc::new(window) as Arc<dyn CaptureSource>,
    ]))
}

fn policy_with_domains(domains: &[&str]) -> PumpPolicy {
    let mut policy = policy(&[]);
    policy.blocked_domains = mc_pipeline::domains::DomainRules::new(
        &domains.iter().map(|d| (*d).to_string()).collect::<Vec<_>>(),
    );
    policy
}

fn policy(blocked_apps: &[&str]) -> PumpPolicy {
    PumpPolicy {
        capture: CapturePolicy {
            interval_secs: 1,
            idle_interval_secs: 10,
            idle_threshold_secs: 5,
            queue_capacity: 16,
            max_parallel_targets: 8,
        },
        hash: HashPolicy::default(),
        blocked_apps: blocked_apps.iter().map(|a| (*a).to_string()).collect(),
        blocked_window_patterns: Vec::new(),
        blocked_domains: mc_pipeline::domains::DomainRules::default(),
        selected_targets: Vec::new(),
        region: None,
    }
}

async fn tick(pump: &mut CapturePump, clock: &TestClock, seconds: u64) {
    clock.advance(std::time::Duration::from_secs(seconds));
    let _ = pump.tick(clock.now(), &CaptureSignals::RUNNING).await;
}

#[tokio::test]
async fn window_metadata_is_recorded_without_an_image() {
    let source = sources("Visual Studio Code", "main.rs - minecontext");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&[]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    tick(&mut pump, &clock, 2).await;

    let window_observations = sink.for_target("window-1");
    assert_eq!(
        window_observations.len(),
        1,
        "窗口元数据必须落成观测，不能被丢弃，实际 {:?}",
        sink.all()
            .iter()
            .map(|o| o.target.id.clone())
            .collect::<Vec<_>>()
    );

    let observation = &window_observations[0];
    assert!(observation.image.is_none(), "元数据观测没有图像");
    assert_eq!(
        observation.target.app_name.as_deref(),
        Some("Visual Studio Code")
    );
    assert_eq!(
        observation.target.window_title.as_deref(),
        Some("main.rs - minecontext")
    );
}

#[tokio::test]
async fn unchanged_window_metadata_is_not_recorded_again() {
    let source = sources("Visual Studio Code", "main.rs - minecontext");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&[]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    for _ in 0..4 {
        tick(&mut pump, &clock, 2).await;
    }

    assert_eq!(
        sink.for_target("window-1").len(),
        1,
        "标题没变就不该每轮都写一行（15s 一条很快会淹掉时间线）"
    );
}

#[tokio::test]
async fn blocked_app_gets_an_audit_row_without_content() {
    let source = sources("1Password", "主密码 - 1Password");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&["1Password"]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    tick(&mut pump, &clock, 2).await;

    let window_observations = sink.for_target("window-1");
    assert_eq!(window_observations.len(), 1, "拦截也要留审计行");
    let observation = &window_observations[0];
    assert!(observation.image.is_none());
    assert_eq!(observation.target.app_name.as_deref(), Some("1Password"));
    assert!(
        observation.target.window_title.is_none(),
        "审计行不留标题（标题可能含敏感内容）"
    );
}

#[tokio::test]
async fn blocked_focused_window_also_blocks_the_screen_frame() {
    let source = sources("1Password", "主密码 - 1Password");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&["1Password"]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    tick(&mut pump, &clock, 2).await;
    tick(&mut pump, &clock, 2).await;

    let screen_observations = sink.for_target("display-1");
    assert!(
        !screen_observations.is_empty(),
        "屏幕观测仍要有（拦住的是内容，不是这一轮本身）"
    );
    assert!(
        screen_observations.iter().all(|o| o.image.is_none()),
        "前台是拉黑应用时，屏幕截图里也全是它的内容 —— 像素不能落盘"
    );
    assert!(
        pump.stats().privacy_blocked >= 2,
        "窗口与屏幕各记一次拦截，实际 {}",
        pump.stats().privacy_blocked
    );
}

#[tokio::test]
async fn unblocked_app_keeps_the_screen_pixels() {
    // 反向探针：没有命中黑名单时不能「顺手」把屏幕帧也拦掉 ——
    // 那会让采集静默失效，而不是 privacy_blocked 计数上升。
    let source = sources("Visual Studio Code", "main.rs - minecontext");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&["1Password"]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    tick(&mut pump, &clock, 2).await;
    tick(&mut pump, &clock, 2).await;

    assert!(
        sink.for_target("display-1")
            .iter()
            .any(|o| o.image.is_some()),
        "没命中黑名单时屏幕帧照常落盘"
    );
    assert_eq!(pump.stats().privacy_blocked, 0);
}

#[tokio::test]
async fn window_observations_carry_their_own_source_id() {
    let source = sources("Visual Studio Code", "main.rs - minecontext");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy(&[]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    tick(&mut pump, &clock, 2).await;

    let window = &sink.for_target("window-1")[0];
    assert_eq!(window.source_kind, SourceKind::Window);
    assert_ne!(window.source_id, sink.for_target("display-1")[0].source_id);
}

#[tokio::test]
async fn blocked_domain_blocks_the_window_and_the_screen_frame() {
    // 端到端：规则不是「解析对了」就算数，要真的拦住观测与像素。
    let source = sources("Google Chrome", "登录 - bank.com");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy_with_domains(&["*.bank.com"]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    tick(&mut pump, &clock, 2).await;

    let window = &sink.for_target("window-1")[0];
    assert!(
        window.target.window_title.is_none(),
        "命中域名规则的窗口不留标题"
    );
    assert!(window.image.is_none());
    assert!(
        sink.for_target("display-1")
            .iter()
            .all(|o| o.image.is_none()),
        "前台窗口命中域名规则时，同轮屏幕帧也不能落盘"
    );
}

#[tokio::test]
async fn unrelated_domain_does_not_block() {
    let source = sources("Google Chrome", "登录 - mybank.example");
    let sink = Arc::new(RecordingSink::default());
    let mut pump = CapturePump::new(
        Arc::clone(&source),
        Arc::new(MemoryFrames::default()),
        Arc::clone(&sink) as Arc<dyn ObservationSink>,
        policy_with_domains(&["*.bank.com"]),
    )
    .unwrap();

    let clock = TestClock::from_millis(T0);
    tick(&mut pump, &clock, 2).await;

    assert_eq!(pump.stats().privacy_blocked, 0, "不相关域名不该被拦");
    assert_eq!(
        sink.for_target("window-1")[0]
            .target
            .window_title
            .as_deref(),
        Some("登录 - mybank.example")
    );
}
