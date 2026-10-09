//! `mc-daemon` — MineContext 的核心守护进程。
//!
//! 职责：加载配置 → 打开数据库 → 绑定 loopback 端口 → 写 `runtime.json`
//! → 提供控制面，并在同一个 tokio 运行时上跑采集环、投影、语义索引与保留策略。
//!
//! 之所以是独立进程而不是嵌进桌面框架：采集必须独立于 UI 存活，
//! 且集成测试可以直接起进程打 HTTP。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use mc_common::error::{AppError, ErrorCode};
use mc_common::observability::{self, LogGuard, LogOptions};
use mc_common::time::{Clock, SystemClock};
use mc_config::{ConfigHandle, LayerSource, LoadRequest};
use mc_server::runtime::{write_runtime_file, RuntimeInfo};
use mc_server::CaptureControls;
use mc_server::{router, ServerState};
use mc_storage::blob::{FileSystemBlobStore, ImageFormat};
use mc_storage::Database;

use mc_common::observability::{error, info, warn};

pub const HELP: &str = "\
mc-daemon — MineContext 上下文引擎守护进程

用法:
  mc-daemon [--config <path>] [--data-dir <path>] [--port <n>]

选项:
  --config <path>    额外的 TOML 配置层（优先级高于内置默认）
  --data-dir <path>  数据目录（默认: 平台标准位置）
  --port <n>         监听端口，0 = 由系统分配（默认: 0）
  -h, --help         显示帮助
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub config_path: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub port: u16,
}

impl Args {
    pub fn with_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_path: None,
            data_dir: data_dir.into(),
            port: 0,
        }
    }
}

pub fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut config_path = None;
    let mut data_dir = None;
    let mut port = 0u16;

    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                let value = iter.next().ok_or("--config 需要一个路径参数")?;
                config_path = Some(PathBuf::from(value));
            }
            "--data-dir" => {
                let value = iter.next().ok_or("--data-dir 需要一个路径参数")?;
                data_dir = Some(PathBuf::from(value));
            }
            "--port" => {
                let value = iter.next().ok_or("--port 需要一个数字参数")?;
                port = value
                    .parse()
                    .map_err(|e| format!("--port 参数非法（{value}）: {e}"))?;
            }
            "-h" | "--help" => return Err(HELP.to_string()),
            other => return Err(format!("未知参数 {other}\n\n{HELP}")),
        }
    }

    Ok(Args {
        config_path,
        data_dir: data_dir.unwrap_or_else(default_data_dir),
        port,
    })
}

/// 平台标准数据目录。
pub fn default_data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("MC_DATA_DIR") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(home).join("Library/Application Support/MineContext")
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(home).join(".minecontext")
    }
}

/// 一次性本地 token。两次 UUIDv4 = 256 bit 随机，足够用于本机鉴权。
pub fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// 启动守护进程，直到收到 Ctrl-C。
pub async fn run(args: Args) -> Result<(), AppError> {
    let guard = init_logging(&args.data_dir);
    info!(
        component = "daemon",
        event = "starting",
        version = env!("CARGO_PKG_VERSION"),
        "mc-daemon 启动"
    );

    let load_request = LoadRequest {
        layers: args
            .config_path
            .iter()
            .cloned()
            .map(LayerSource::File)
            .collect(),
        env: Vec::new(),
        read_process_env: true,
    };
    let data_dir = args.data_dir.clone();
    let (load_request, user_config) = resolve_user_config(load_request, &args, &data_dir)?;
    let loaded = mc_config::load(&load_request)?;
    info!(
        component = "config",
        event = "loaded",
        layers = load_request.layers.len(),
        "配置已加载"
    );

    std::fs::create_dir_all(data_dir.join("data")).map_err(|e| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法创建数据目录 {}: {e}", data_dir.display()),
        )
    })?;

    let db_path = data_dir.join("data").join("minecontext.db");
    let db = Arc::new(Database::open(&db_path)?);
    info!(component = "storage", event = "opened", "事件库已打开");

    let token = generate_token();
    let started_at = SystemClock.now();

    // 挂载采集控制：blob 落盘 + 平台采集源。
    // 即使当前没有屏幕录制权限也照常挂载 —— 让 HTTP 层如实报告
    // 「不可用 + 去哪里授权」，而不是让接口消失。
    let blobs = Arc::new(FileSystemBlobStore::new(
        data_dir.join("blobs"),
        ImageFormat::Png,
    )?);
    // 跑哪些源由 `capture.sources` 决定。配置里点名了没实现的源时：
    // 如实报告并跳过，绝不悄悄退回「截全部屏幕」。
    let selection = mc_capture::platform::sources_for(&loaded.config.capture.sources)?;
    if !selection.ignored.is_empty() {
        warn!(
            component = "capture",
            event = "sources_ignored",
            names = %selection.ignored.join(","),
            "有采集源尚未实现，已跳过（可用：screen、window）"
        );
    }
    let controls = Arc::new(CaptureControls::new(selection.source, blobs));

    let state = Arc::new(
        ServerState::new(
            ConfigHandle::new(loaded),
            Arc::clone(&db),
            token.clone(),
            started_at,
            data_dir.clone(),
        )
        .with_capture(controls)
        // 设置类接口要能把改动写到用户配置层并热重载
        .with_config_write(load_request, user_config),
    );

    let address = SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(address).await.map_err(|e| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法监听 {address}: {e}"),
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| AppError::new(ErrorCode::StorageUnavailable, format!("无法获取端口: {e}")))?
        .port();

    let runtime_path = write_runtime_file(
        &data_dir,
        &RuntimeInfo {
            port,
            token,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started_at,
        },
    )?;

    info!(
        component = "server",
        event = "listening",
        port = port,
        "控制面已监听 127.0.0.1"
    );

    // 崩溃恢复：上次被 kill -9 时留下的 open 阶段要补记为 interrupted，
    // 否则它会永远挂在「进行中」，既不会有总结，也不会被巡检看到。
    let recovered = mc_server::stages::recover_interrupted(&state, SystemClock.now())
        .unwrap_or_else(|failure| {
            error!(
                component = "stages",
                event = "recover_failed",
                detail = %observability::error_summary(&failure),
                "恢复中断阶段失败"
            );
            0
        });
    if recovered > 0 {
        info!(
            component = "stages",
            event = "recovered",
            count = recovered,
            "补记被中断的阶段"
        );
    }

    // 采集环：仅在「用户上次要录」且本机就绪时自动恢复。
    // 用户点过停止后 capture.enabled=false，重开不得再自动开录；
    // 未就绪时也不要把 running 置 true（否则 UI 会「停止录制」+「缺少权限」并存）。
    if let Some(controls) = state.capture.as_ref() {
        let enabled = loaded_capture_enabled(&state);
        let ready = mc_capture::platform::probe_readiness().available;
        if should_auto_start_capture(enabled, ready) {
            controls.start();
            info!(
                component = "capture",
                event = "started",
                interval_secs = interval_secs(&state),
                source = controls.source_id(),
                "采集已启动"
            );
        } else {
            info!(
                component = "capture",
                event = "not_auto_started",
                enabled = enabled,
                ready = ready,
                "启动时未自动开录（尊重上次停止或本机未就绪）"
            );
        }
    }
    // 保留策略轮转：按 capture.retention_days 删旧图并清理悬空引用。
    // 它是磁盘占用唯一的下降路径 —— 不跑就只有涨，因此与 daemon 同生命周期。
    let retention = mc_server::retention::spawn_retention_task(
        Arc::clone(&state),
        mc_server::retention::DEFAULT_INTERVAL,
    );

    // 作业消费者：队列里的补偿作业由它执行（没有它，入队的作业永远不会被处理）。
    let jobs = mc_server::jobs_worker::spawn_jobs_worker(Arc::clone(&state));

    let capture = mc_server::capture_loop::spawn_capture_loop(
        Arc::clone(&state),
        std::time::Duration::from_millis(500),
    );

    // 后台投影：观测进日志后自动产出活动与阶段。
    // 没有这个循环，时间线会永远停在「有截图、没有活动」。
    let projector = mc_server::activities::spawn_projector(Arc::clone(&state));

    // 语义索引：**独立任务**，与投影循环互不阻塞。
    // 索引失败（未配置 / 限流 / 维度变化）不会影响采集、投影与总结，
    // 只是让检索退化为关键词（见 `mc-server::embedding`）。
    let indexer = mc_server::embedding::spawn_worker(Arc::clone(&state));

    info!(
        component = "pipeline",
        event = "workers_started",
        "投影、语义索引与保留策略循环已启动"
    );

    let result = axum::serve(listener, router(Arc::clone(&state)))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| AppError::new(ErrorCode::StorageUnavailable, format!("服务异常退出: {e}")));

    projector.abort();
    indexer.abort();
    capture.abort();
    retention.abort();
    jobs.abort();

    // 关机收尾：立刻关掉当前阶段并写一份兜底总结。
    // 刻意不等模型：退出路径必须有时间上界，兜底是确定性的、亚毫秒级。
    match mc_server::stages::flush_on_shutdown(
        &state,
        &mc_server::stages::policy_for(&state),
        SystemClock.now(),
    )
    .await
    {
        Ok(Some(id)) => info!(
            component = "stages",
            event = "shutdown_summary",
            id = %observability::redact_id(&id),
            "已为当前阶段写入总结"
        ),
        Ok(None) => {}
        Err(failure) => error!(
            component = "stages",
            event = "shutdown_summary_failed",
            detail = %observability::error_summary(&failure),
            "关机收尾失败"
        ),
    }

    // 退出时清理 runtime.json，避免前端读到过期端口
    let _ = std::fs::remove_file(&runtime_path);
    info!(component = "daemon", event = "stopped", "已退出");
    guard.flush();
    result
}

/// 设置日志落点：文件不可用时退回只写 stderr，绝不因为日志起不来。
///
/// 日志目录放在数据目录下，但**日志里不写这个路径** —— 支持包是要发给用户的。
fn init_logging(data_dir: &std::path::Path) -> LogGuard {
    let options = LogOptions::from_env().with_file(data_dir.join("logs").join("daemon.log"));
    observability::init(options).unwrap_or_else(|failure| {
        eprintln!(
            "文件日志不可用，仅写 stderr：{}",
            observability::error_summary(&failure)
        );
        observability::init(LogOptions::from_env()).expect("stderr 落点必须可用")
    })
}

/// 等待退出信号。
///
/// **SIGTERM 也要处理**：进程管理器、`kill`、容器停止发的都是 SIGTERM，
/// 只处理 Ctrl-C（SIGINT）的话，那些场景下不会走到清理路径 ——
/// `runtime.json` 会留在磁盘上，前端下次启动读到的是**过期端口**。
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// 配置里是否允许采集（`capture.enabled`）。
fn loaded_capture_enabled(state: &std::sync::Arc<mc_server::ServerState>) -> bool {
    state.config.current().config.capture.enabled
}

/// 启动时是否自动开录：必须同时「用户要录」且「本机可录」。
///
/// 拆成纯函数是为了钉住「停止后重开不得自动开录」与「缺权限不得谎报 running」。
pub fn should_auto_start_capture(enabled: bool, ready: bool) -> bool {
    enabled && ready
}

/// 采集间隔（秒），用于启动日志。
fn interval_secs(state: &std::sync::Arc<mc_server::ServerState>) -> u64 {
    state.config.current().config.capture.interval_secs
}

/// 解析用户配置文件的位置，并把它接进分层加载。
///
/// - 显式 `--config <path>`：**必须存在**（路径拼错要立刻报错，而不是静默用默认值跑）；
/// - 未显式指定：用 `<data_dir>/config.toml`，不存在就创建一个空文件 ——
///   用户还没改过设置时它是空的，改过之后重启仍然生效。
fn resolve_user_config(
    mut request: LoadRequest,
    args: &Args,
    data_dir: &std::path::Path,
) -> Result<(LoadRequest, std::path::PathBuf), AppError> {
    if let Some(path) = args.config_path.clone() {
        return Ok((request, path));
    }

    let path = data_dir.join("config.toml");
    if !path.exists() {
        mc_config::write::write_atomic(&path, "")?;
    }
    if !request
        .layers
        .iter()
        .any(|layer| matches!(layer, LayerSource::File(existing) if existing == &path))
    {
        request.layers.push(LayerSource::File(path.clone()));
    }
    Ok((request, path))
}
