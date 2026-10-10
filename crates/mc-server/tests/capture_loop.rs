//! 把采集环真正接进 daemon。
//!
//! `CapturePump`（调度 + 变化检测 + 隐私 + 落盘 + 落库）本身有测试，但必须有东西真的在跑它：
//! 产品最核心的那条循环（每 N 秒看一眼屏幕、有变化才落盘）要在真实运行里发生。钉住四条行为：
//! 1. 定时真的会产出观测（不是「接口在、采集没发生」）；
//! 2. `stop()` 之后立刻停（用户点暂停就该停）；
//! 3. 采集源报错不能让循环退出（临时失败不能变成永久停摆）；
//! 4. 间隔来自配置（改了配置就改节奏）。

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mc_capture::scheduler::CaptureSignals;
use mc_capture::source::{
    CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities, SourceHealth,
    SourceKind,
};
use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_server::{capture_loop, router, CaptureControls, ServerState};
use mc_storage::blob::{FileSystemBlobStore, ImageFormat};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

/// 受控的假采集源：每次 poll 返回一张**不同**的图（否则会被变化检测判为「没变」）。
struct ScriptedSource {
    calls: std::sync::atomic::AtomicUsize,
    fail_until: usize,
}

impl ScriptedSource {
    fn new(fail_until: usize) -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            fail_until,
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CaptureSource for ScriptedSource {
    fn id(&self) -> &str {
        "screen:display-1"
    }

    fn kind(&self) -> mc_capture::source::SourceKind {
        mc_capture::source::SourceKind::Screen
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::SCREEN
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, mc_common::error::AppError> {
        Ok(vec![CaptureTarget::screen("display-1", "Display 1", 1.0)])
    }

    async fn poll(
        &self,
        ctx: &mc_capture::source::CaptureContext,
    ) -> Result<Vec<RawCapture>, mc_common::error::AppError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call < self.fail_until {
            return Err(mc_common::error::AppError::new(
                ErrorCode::CaptureIo,
                "模拟采集失败",
            ));
        }

        // 每次给一张不同的图：整幅灰阶随 call 变化（否则会被变化检测判为「没变」）
        let shade = (call as u8).wrapping_mul(17).max(8);
        let image = image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([
                shade.wrapping_add(x as u8),
                shade.wrapping_add(y as u8),
                ((x + y) as u8).wrapping_mul(3),
            ])
        });

        // 目标 id 由调度器给出
        let target = mc_capture::source::CaptureTarget::screen(
            ctx.targets
                .first()
                .cloned()
                .unwrap_or_else(|| "display-1".to_string()),
            "Display 1",
            1.0,
        );

        Ok(vec![RawCapture {
            source_id: "fake:screen".to_string(),
            source_kind: SourceKind::Screen,
            target,
            captured_at: ctx.now,
            image: Some(image),
            text: None,
        }])
    }

    async fn health(&self) -> SourceHealth {
        SourceHealth {
            available: true,
            permission: PermissionState::Granted,
            message: None,
        }
    }
}

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
    source: Arc<ScriptedSource>,
}

fn ctx(interval_secs: u64, fail_until: usize) -> Ctx {
    ctx_toml(
        &format!("[capture]\ninterval_secs = {interval_secs}\n"),
        fail_until,
    )
}

fn ctx_toml(toml: &str, fail_until: usize) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "test".to_string(),
            toml: toml.to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .unwrap();

    let blobs = Arc::new(
        FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).expect("blob store"),
    );
    let source = ScriptedSource::new(fail_until);
    let controls = Arc::new(CaptureControls::new(
        Arc::clone(&source) as Arc<dyn CaptureSource>,
        blobs,
    ));

    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(config),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(T0),
            dir.path().to_path_buf(),
        )
        .with_capture(controls),
    );

    Ctx {
        _dir: dir,
        state,
        source,
    }
}

fn observations(state: &ServerState) -> usize {
    state.db.observation_count().unwrap() as usize
}

// ---------------------------------------------------------------- 定时采集

/// 测试用：注入固定信号（不锁屏、不空闲）。
///
/// 直接跑生产版的 `spawn_capture_loop` 会读开发机的真实空闲时长，而调度器
/// 在空闲时会把节奏降到 30 秒一次 —— 于是「几秒内应该采到两帧」的断言会
/// 随开发机有没有输入而红。信号来源必须能从外部替换。
fn spawn_loop_deterministic(
    state: Arc<ServerState>,
    tick: Duration,
) -> tokio::task::JoinHandle<()> {
    capture_loop::spawn_capture_loop_with_signals(state, tick, Arc::new(|| CaptureSignals::RUNNING))
}

#[tokio::test]
async fn the_loop_actually_captures_on_its_own() {
    let ctx = ctx(1, 0);
    ctx.state.capture.as_ref().unwrap().start();

    let handle = spawn_loop_deterministic(Arc::clone(&ctx.state), Duration::from_millis(30));

    // 给它几轮时间（实际间隔由 policy 决定，这里只等结果）
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while observations(&ctx.state) < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    handle.abort();

    assert!(
        observations(&ctx.state) >= 2,
        "daemon 必须自己产出观测（而不是只有 /api/capture/now 按需截图）；实际 {}",
        observations(&ctx.state)
    );
    assert!(ctx.source.calls() >= 2, "采集源必须被反复调用");

    // 落库的观测要带图片引用（否则「有记录、没截图」）
    let rows = ctx
        .state
        .db
        .query_observations(&mc_storage::observations::ObservationQuery::default())
        .unwrap();
    assert!(
        rows.iter().any(|row| row.image_path.is_some()),
        "观测必须带截图路径：{rows:?}"
    );
}

#[tokio::test]
async fn stopping_the_controls_stops_the_capture() {
    let ctx = ctx(1, 0);
    ctx.state.capture.as_ref().unwrap().start();
    let handle = spawn_loop_deterministic(Arc::clone(&ctx.state), Duration::from_millis(20));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while observations(&ctx.state) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(observations(&ctx.state) > 0, "前置条件：先要有采集发生");

    // 用户点「暂停」
    ctx.state.capture.as_ref().unwrap().stop();
    tokio::time::sleep(Duration::from_millis(120)).await;
    let frozen = observations(&ctx.state);

    tokio::time::sleep(Duration::from_millis(150)).await;
    handle.abort();

    assert_eq!(observations(&ctx.state), frozen, "暂停之后不该再有新的观测");
}

/// 临时故障不能让采集永久停摆（局部故障不停摆）
#[tokio::test]
async fn a_failing_source_does_not_kill_the_loop() {
    let ctx = ctx(1, 2); // 前两次 poll 失败
    ctx.state.capture.as_ref().unwrap().start();
    let handle = spawn_loop_deterministic(Arc::clone(&ctx.state), Duration::from_millis(25));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while observations(&ctx.state) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    handle.abort();

    assert!(
        observations(&ctx.state) > 0,
        "前两次失败之后必须继续重试并成功（循环不能退出）"
    );

    // 失败要可见：诊断页能查到采集失败
    let failures: i64 = ctx
        .state
        .db
        .with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pipeline_failures WHERE component = 'capture'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert!(failures >= 1, "采集失败必须写进 pipeline_failures");
}

/// 间隔来自配置：大间隔时只会有「第一次立即采集」，不会反复采集。
///
/// 第一次立即采集是调度器的有意行为（没有上一帧时没理由等一个间隔），
/// 因此这里断言的是**不重复**，而不是「一次都没有」。
#[tokio::test]
async fn the_interval_comes_from_config() {
    let slow = ctx(3600, 0);
    slow.state.capture.as_ref().unwrap().start();
    let handle = spawn_loop_deterministic(Arc::clone(&slow.state), Duration::from_millis(20));
    tokio::time::sleep(Duration::from_millis(250)).await;
    handle.abort();
    let slow_count = observations(&slow.state);

    let fast = ctx(1, 0);
    fast.state.capture.as_ref().unwrap().start();
    let handle = spawn_loop_deterministic(Arc::clone(&fast.state), Duration::from_millis(20));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while observations(&fast.state) < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    handle.abort();
    let fast_count = observations(&fast.state);

    assert!(
        slow_count <= 1,
        "间隔 1 小时时 250ms 内最多只有第一次采集，实际 {slow_count}"
    );
    assert!(
        fast_count >= 2,
        "间隔 1 秒时应当在几秒内采集多次，实际 {fast_count}"
    );
}

fn request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(Body::empty())
        .unwrap()
}

/// 采集开关与 HTTP 面的状态必须一致（前端靠 `/api/capture/status` 显示录制中）
#[tokio::test]
async fn status_endpoint_reflects_the_loop_state() {
    let ctx = ctx(1, 0);
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request("GET", "/api/capture/status"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(envelope["data"]["status"], "stopped", "初始未录制");

    ctx.state.capture.as_ref().unwrap().start();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request("GET", "/api/capture/status"))
        .await
        .unwrap();
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        envelope["data"]["status"], "running",
        "start 之后必须是录制中"
    );
}

/// 诊断页必须能看到「采集到底做了什么」——否则「采集好像在工作」永远只是猜。
#[tokio::test]
async fn diagnostics_report_capture_stats() {
    let ctx = ctx(1, 0);
    ctx.state.capture.as_ref().unwrap().start();
    let handle = spawn_loop_deterministic(Arc::clone(&ctx.state), Duration::from_millis(20));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while observations(&ctx.state) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle.abort();

    let stats = ctx
        .state
        .capture_stats()
        .expect("跑过采集轮之后必须有统计快照");
    assert!(stats.persisted >= 1, "{stats:?}");
    assert_eq!(stats.dropped, 0, "dropped 必须恒为 0：{stats:?}");
}

// ---------------------------------------------------------------- 锁屏/休眠推送

/// 信号可变的采集环：用来验证「状态变化 → 推事件」这条链路。
fn spawn_loop_with_shared_signals(
    state: Arc<ServerState>,
    tick: Duration,
    signals: Arc<std::sync::Mutex<CaptureSignals>>,
) -> tokio::task::JoinHandle<()> {
    capture_loop::spawn_capture_loop_with_signals(
        state,
        tick,
        Arc::new(move || *signals.lock().unwrap()),
    )
}

#[tokio::test]
async fn lock_state_changes_are_pushed_to_the_renderer() {
    let ctx = ctx(1, 0);
    ctx.state.capture.as_ref().unwrap().start();
    let signals = Arc::new(std::sync::Mutex::new(CaptureSignals::RUNNING));
    let mut events = ctx.state.events.subscribe();

    let handle = spawn_loop_with_shared_signals(
        Arc::clone(&ctx.state),
        Duration::from_millis(20),
        Arc::clone(&signals),
    );

    // 先让循环读到一次「未锁屏」：首次读到不发事件（否则每次启动都推一条假消息）
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        events.try_recv().is_err(),
        "首次读到状态不该推事件（避免启动时推假消息）"
    );

    // 锁屏 → 应当推 lock-screen
    *signals.lock().unwrap() = CaptureSignals {
        locked: true,
        suspended: false,
        idle_for_secs: 0,
    };
    let locked = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(event) = events.recv().await {
                if event.kind == "push:power-monitor" {
                    return event;
                }
            }
        }
    })
    .await
    .expect("锁屏后必须推 push:power-monitor");
    assert_eq!(
        locked.data["eventKey"], "lock-screen",
        "载荷形状必须与渲染层对齐（eventKey/data）：{}",
        locked.data
    );

    // 解锁 → 应当推 unlock-screen（同一状态反复读不重复推）
    *signals.lock().unwrap() = CaptureSignals::RUNNING;
    let unlocked = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(event) = events.recv().await {
                if event.kind == "push:power-monitor" {
                    return event;
                }
            }
        }
    })
    .await
    .expect("解锁后必须推 push:power-monitor");
    assert_eq!(unlocked.data["eventKey"], "unlock-screen");

    handle.abort();
}

/// 默认开启锁屏暂停：锁屏期间不得新增观测，且不改 `capture.enabled`。
#[tokio::test]
async fn lock_pauses_capture_by_default_without_clearing_enabled() {
    let ctx = ctx(1, 0);
    assert!(
        ctx.state.config.current().config.capture.pause_on_lock,
        "默认必须开启锁屏暂停"
    );
    ctx.state.capture.as_ref().unwrap().start();
    let signals = Arc::new(std::sync::Mutex::new(CaptureSignals::RUNNING));
    let handle = spawn_loop_with_shared_signals(
        Arc::clone(&ctx.state),
        Duration::from_millis(20),
        Arc::clone(&signals),
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while observations(&ctx.state) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(observations(&ctx.state) > 0, "前置：先采到至少一帧");

    *signals.lock().unwrap() = CaptureSignals {
        locked: true,
        // 与空闲降频组合：锁屏优先，空闲再长也不该采
        idle_for_secs: 600,
        suspended: false,
    };
    tokio::time::sleep(Duration::from_millis(80)).await;
    let frozen = observations(&ctx.state);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        observations(&ctx.state),
        frozen,
        "锁屏（默认 pause_on_lock）期间不得新增观测"
    );
    assert!(
        ctx.state.config.current().config.capture.enabled,
        "锁屏不得改写 capture.enabled；解锁后按原开启状态继续"
    );
    assert!(
        ctx.state.capture.as_ref().unwrap().is_running(),
        "锁屏不得 stop 采集控制"
    );

    *signals.lock().unwrap() = CaptureSignals::RUNNING;
    let resume_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while observations(&ctx.state) <= frozen && tokio::time::Instant::now() < resume_deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle.abort();
    assert!(
        observations(&ctx.state) > frozen,
        "解锁后必须在原 enabled/running 状态下继续采集"
    );
}

/// 用户关掉 pause_on_lock 后，锁屏不再硬暂停（空闲降频仍可生效）。
#[tokio::test]
async fn lock_does_not_pause_when_pause_on_lock_is_disabled() {
    let ctx = ctx_toml(
        "[capture]\ninterval_secs = 1\npause_on_lock = false\n",
        0,
    );
    assert!(!ctx.state.config.current().config.capture.pause_on_lock);
    ctx.state.capture.as_ref().unwrap().start();
    let signals = Arc::new(std::sync::Mutex::new(CaptureSignals {
        locked: true,
        suspended: false,
        idle_for_secs: 0,
    }));
    let handle = spawn_loop_with_shared_signals(
        Arc::clone(&ctx.state),
        Duration::from_millis(20),
        Arc::clone(&signals),
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while observations(&ctx.state) < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle.abort();
    assert!(
        observations(&ctx.state) >= 2,
        "pause_on_lock=false 时锁屏仍应继续采集，实际 {}",
        observations(&ctx.state)
    );
}
