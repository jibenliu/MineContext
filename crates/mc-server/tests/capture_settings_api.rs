//! 采集设置写回：`targets/selection` 与 `PATCH /api/capture/config`。
//!
//! 这一组接口是「UI 改设置 → 用户配置层 → 热重载」的入口，
//! 也是之前刻意没做的部分：当时没有配置写回路径，硬做会变成
//! 「接口收下了、行为没变」的假实现。现在写回路径已经落好，因此可以接上，
//! 并且必须验证**行为真的变了**，而不只是文件里多了几行。

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_capture::scheduler::CaptureSignals;
use mc_capture::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind,
};
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_server::{capture_loop, router, CaptureControls, ServerState};
use mc_storage::blob::{FileSystemBlobStore, ImageFormat};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

/// 两屏假源：选择逻辑要能区分它们。
struct TwoScreenSource;

#[async_trait::async_trait]
impl CaptureSource for TwoScreenSource {
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
        Ok(vec![
            CaptureTarget::screen("display-1", "Display 1", 2.0),
            CaptureTarget::screen("display-2", "Display 2", 1.0),
        ])
    }
    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        Ok(ctx
            .targets
            .iter()
            .map(|id| RawCapture {
                source_id: "fake:screen".to_string(),
                source_kind: SourceKind::Screen,
                target: CaptureTarget::screen(id.clone(), id.clone(), 1.0),
                captured_at: ctx.now,
                image: Some(image::RgbImage::from_pixel(
                    32,
                    24,
                    image::Rgb([
                        (id.len() * 31) as u8,
                        (ctx.now.as_millis() % 251) as u8,
                        120,
                    ]),
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

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
    config_path: std::path::PathBuf,
}

fn ctx(initial_config: &str) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, initial_config).unwrap();

    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let request = mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(config_path.clone())],
        env: Vec::new(),
        read_process_env: false,
    };
    let loaded = mc_config::load::load(&request).unwrap();

    let blobs = Arc::new(
        FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).expect("blob store"),
    );
    let state = ServerState::new(
        mc_config::ConfigHandle::new(loaded),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    )
    .with_capture(Arc::new(CaptureControls::new(
        Arc::new(TwoScreenSource) as Arc<dyn CaptureSource>,
        blobs,
    )))
    .with_config_write(request, config_path.clone());

    Ctx {
        _dir: dir,
        state: Arc::new(state),
        config_path,
    }
}

fn request(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(request(method, uri, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("必须返回 JSON 信封")
}

// ---------------------------------------------------------------- 选择显示器

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
async fn selection_is_persisted_and_visible_in_targets() {
    let ctx = ctx("");

    // 前端发的是 CaptureSource[]（对象数组），这里带一个 id
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/capture/targets/selection",
        Some(serde_json::json!([{ "id": "display-2", "name": "Display 2", "type": "screen" }])),
    )
    .await;
    assert_eq!(envelope["data"]["success"], true, "{envelope}");

    // 落盘
    let text = std::fs::read_to_string(&ctx.config_path).unwrap();
    assert!(text.contains("display-2"), "选择必须写进用户配置：{text}");

    // 热生效（不用重启）
    assert_eq!(
        ctx.state.config.current().config.capture.target_ids,
        vec!["display-2".to_string()]
    );

    // 接口如实报告
    let items = call(&ctx.state, "GET", "/api/capture/targets", None).await["data"]
        .as_array()
        .unwrap()
        .clone();
    let selected: Vec<&str> = items
        .iter()
        .filter(|item| item["selected"] == true)
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(selected, vec!["display-2"]);
}

/// 替换语义：累积（`uniqBy([...appInfo, ...appInfo])`），
/// 于是**取消勾选永远不生效**。这里明确是替换。
#[tokio::test]
async fn selection_replaces_the_previous_one() {
    let ctx = ctx("[capture]\ntarget_ids = [\"display-1\"]\n");

    call(
        &ctx.state,
        "POST",
        "/api/capture/targets/selection",
        Some(serde_json::json!([{ "id": "display-2" }])),
    )
    .await;

    assert_eq!(
        ctx.state.config.current().config.capture.target_ids,
        vec!["display-2".to_string()],
        "新选择必须替换旧的（累积语义会让取消勾选失效）"
    );
}

/// 清空选择 = 恢复「全部可见目标」
#[tokio::test]
async fn empty_selection_means_all_targets_again() {
    let ctx = ctx("[capture]\ntarget_ids = [\"display-1\"]\n");

    call(
        &ctx.state,
        "POST",
        "/api/capture/targets/selection",
        Some(serde_json::json!([])),
    )
    .await;

    assert!(
        ctx.state
            .config
            .current()
            .config
            .capture
            .target_ids
            .is_empty(),
        "空选择 = 不限制"
    );
}

// ---------------------------------------------------------------- 采集设置

#[tokio::test]
async fn patch_capture_config_updates_interval_and_hours() {
    let ctx = ctx("");

    let envelope = call(
        &ctx.state,
        "PATCH",
        "/api/capture/config",
        Some(serde_json::json!({
            "recordInterval": 5,
            "enableRecordingHours": true,
            "recordingHours": ["08:00:00", "20:00:00"],
            "applyToDays": "weekday"
        })),
    )
    .await;
    assert_eq!(envelope["data"]["success"], true, "{envelope}");

    let config = ctx.state.config.current();
    assert_eq!(config.config.capture.interval_secs, 5);
    assert!(config.config.capture.enable_recording_hours);
    assert_eq!(
        config.config.capture.recording_hours,
        Some(["08:00:00".to_string(), "20:00:00".to_string()])
    );
    assert_eq!(
        config.config.capture.apply_to_days,
        mc_config::model::ApplyToDays::Weekday
    );
}

#[tokio::test]
async fn invalid_patch_is_rejected_and_config_is_unchanged() {
    let ctx = ctx("[capture]\ninterval_secs = 15\n");

    let envelope = call(
        &ctx.state,
        "PATCH",
        "/api/capture/config",
        Some(serde_json::json!({ "unknownField": 1 })),
    )
    .await;
    assert_eq!(envelope["code"], 1, "未知字段必须被拒绝：{envelope}");
    assert_eq!(ctx.state.config.current().config.capture.interval_secs, 15);
    assert_eq!(
        std::fs::read_to_string(&ctx.config_path).unwrap(),
        "[capture]\ninterval_secs = 15\n",
        "被拒绝的补丁不能改动文件"
    );
}

/// 未挂载可写配置的实例（只读/无用户配置文件）必须给出结构化错误
#[tokio::test]
async fn without_a_writable_config_it_fails_structurally() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));

    let envelope = call(
        &state,
        "PATCH",
        "/api/capture/config",
        Some(serde_json::json!({ "recordInterval": 5 })),
    )
    .await;
    assert_eq!(envelope["code"], 1);
    assert_eq!(envelope["error_code"], "config_unreadable");
}

// ---------------------------------------------------------------- 行为真的变了

/// 时段设置必须**真的**让采集停下来 —— 只写进配置不算完成。
#[tokio::test]
async fn recording_hours_actually_gate_the_capture_loop() {
    // 把时段设成「当前时间 +1h ~ +2h」：现在必然在时段外
    let now = chrono::Local::now();
    let start = (now + chrono::Duration::hours(1))
        .format("%H:%M:%S")
        .to_string();
    let end = (now + chrono::Duration::hours(2))
        .format("%H:%M:%S")
        .to_string();

    let ctx = ctx(&format!(
        "[capture]\ninterval_secs = 1\nenable_recording_hours = true\nrecording_hours = [\"{start}\", \"{end}\"]\napply_to_days = \"everyday\"\n"
    ));
    ctx.state.capture.as_ref().unwrap().start();

    let handle = spawn_loop_deterministic(Arc::clone(&ctx.state), Duration::from_millis(20));
    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.abort();

    assert_eq!(
        ctx.state.db.observation_count().unwrap(),
        0,
        "时段之外不该有任何采集（配置写了但行为没变是最糟的结果）"
    );
}

/// 选择改变后，采集环必须立刻按新选择工作（热生效）
#[tokio::test]
async fn selection_change_takes_effect_on_the_running_loop() {
    let ctx = ctx("[capture]\ninterval_secs = 1\ntarget_ids = [\"display-1\"]\n");
    ctx.state.capture.as_ref().unwrap().start();
    let handle = spawn_loop_deterministic(Arc::clone(&ctx.state), Duration::from_millis(20));

    // 先让它在 display-1 上采集一会儿
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while ctx.state.db.observation_count().unwrap() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ctx.state.db.observation_count().unwrap() > 0, "前置条件");

    // 改成 display-2
    call(
        &ctx.state,
        "POST",
        "/api/capture/targets/selection",
        Some(serde_json::json!([{ "id": "display-2" }])),
    )
    .await;

    // 清掉已有观测，观察新采集落在哪个目标上
    ctx.state
        .db
        .with_write(|conn| {
            conn.execute("DELETE FROM observations", [])?;
            Ok(())
        })
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut seen: Vec<String> = Vec::new();
    while seen.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
        seen = ctx
            .state
            .db
            .query_observations(&mc_storage::observations::ObservationQuery::default())
            .unwrap()
            .iter()
            .map(|row| {
                row.display_id
                    .clone()
                    .unwrap_or_else(|| row.source_id.clone())
            })
            .collect();
    }
    handle.abort();

    assert!(
        seen.iter().all(|id| id.contains("display-2")),
        "切换选择后只应采集 display-2，实际 {seen:?}"
    );
}
