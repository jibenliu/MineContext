//! 采集环接线：定时采集、变化检测、隐私判定、落盘与落库。
//!
//! 这一层把三样生产实现接在一起，并集中在一处：`FrameStore`（`FileSystemBlobStore`）、
//! `ObservationSink`（`Database::insert_observation`，观测与事件同事务、幂等）、
//! 以及循环本身（间隔来自配置、开关来自 `CaptureControls`、
//! 失败写 `pipeline_failures` 但**绝不退出**）。
//!
//! 每 tick 的锁屏 / 空闲信号由 [`SignalsSource`] 提供：生产读系统（[`system_signals`]），
//! 测试注入固定值 —— 否则结果会随开发机是否空闲而变；探测读不出来时按「未锁屏、
//! 不空闲」处理（宁可多采，也不因探测失败整体停采）。

use std::sync::Arc;
use std::time::Duration;

use image::RgbImage;
use mc_capture::scheduler::{CapturePolicy, CaptureSignals};
use mc_common::error::AppError;
use mc_common::observability::{debug, error, error_summary, warn};
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_pipeline::pump::{
    CapturePump, CapturedObservation, FrameStore, ObservationSink, PrivacyVerdict, PumpPolicy,
    StoredFrame,
};
use mc_storage::blob::{BlobStore, FileSystemBlobStore, ImageMeta};
use mc_storage::observations::{ImageRef, NewObservation};
use mc_storage::Database;

use crate::failures::record_capture_failure;
use crate::state::ServerState;

/// 默认循环节奏（多久看一眼「该不该采集了」）。
///
/// 它**不是**采集间隔 —— 采集间隔由 `CapturePolicy::interval_secs` 决定。
/// 循环节奏只影响「采集时刻的准时程度」，因此取一个明显小于间隔的值。
pub const DEFAULT_TICK: Duration = Duration::from_millis(500);

/// 帧存储：把内存里的帧编码后写进 blob 目录。
pub struct BlobFrameStore {
    blobs: Arc<FileSystemBlobStore>,
    display_id: Option<String>,
}

impl BlobFrameStore {
    pub fn new(blobs: Arc<FileSystemBlobStore>, display_id: Option<String>) -> Self {
        Self { blobs, display_id }
    }
}

impl FrameStore for BlobFrameStore {
    fn store_frame(&self, image: &RgbImage, at: Timestamp) -> Result<StoredFrame, AppError> {
        let stored = self.blobs.put_image(
            image,
            &ImageMeta {
                captured_at: at,
                display_id: self.display_id.clone(),
            },
        )?;

        Ok(StoredFrame {
            relative_path: stored.relative_path,
            content_hash: stored.content_hash,
            thumbnail_path: stored.thumbnail.map(|thumb| thumb.relative_path),
            width: stored.width,
            height: stored.height,
            bytes: stored.bytes,
        })
    }
}

/// 观测去向：写库（观测 + 事件同事务 + 幂等）。
pub struct DatabaseSink {
    db: Arc<Database>,
}

impl DatabaseSink {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl ObservationSink for DatabaseSink {
    fn persist(&self, observation: &CapturedObservation) -> Result<(), AppError> {
        let image = observation.image.as_ref().map(|frame| ImageRef {
            relative_path: frame.relative_path.clone(),
            content_hash: frame.content_hash.clone(),
            thumbnail_path: frame.thumbnail_path.clone(),
            width: frame.width,
            height: frame.height,
            bytes: frame.bytes,
        });

        self.db.insert_observation(&NewObservation {
            id: observation.id.clone(),
            ts: observation.captured_at,
            source_id: observation.source_id.clone(),
            kind: observation.source_kind.as_str().to_string(),
            app_name: observation.target.app_name.clone(),
            app_bundle_id: None,
            window_title: observation.target.window_title.clone(),
            domain: None,
            display_id: observation.target.display_id.clone(),
            scale_factor: Some(observation.target.scale_factor),
            image,
            text_content: None,
            text_origin: None,
            change_kind: change_kind_str(observation.change_kind).to_string(),
            privacy_verdict: privacy_str(observation.privacy_verdict).to_string(),
            phash: Some(observation.phash),
            idempotency: observation.idempotency.clone(),
        })?;
        Ok(())
    }
}

fn change_kind_str(kind: mc_capture::change::ChangeKind) -> &'static str {
    use mc_capture::change::ChangeKind;
    match kind {
        ChangeKind::New => "new",
        ChangeKind::TitleOnly => "title_only",
        ChangeKind::PixelMinor => "pixel_minor",
        ChangeKind::PixelMajor => "pixel_major",
        ChangeKind::Idle => "idle",
        ChangeKind::Unknown => "unknown",
    }
}

fn privacy_str(verdict: PrivacyVerdict) -> &'static str {
    match verdict {
        PrivacyVerdict::Allowed => "allowed",
        PrivacyVerdict::Redacted => "redacted",
        PrivacyVerdict::Blocked => "blocked",
    }
}

/// 从配置生成泵策略。
///
/// 上限与阈值全部来自配置 —— 写死的话，用户在设置里改采集频率不会有任何效果。
pub fn policy_from(config: &mc_config::Config) -> PumpPolicy {
    // 域名规则已配置却拿不到 URL：匹配只能按标题启发式，如实报告而不是静默降级
    if let Some(notice) = crate::domain_rules::notice(&config.privacy.blocked_domains) {
        warn!("{notice}");
    }
    let capture = &config.capture;
    PumpPolicy {
        capture: CapturePolicy {
            interval_secs: capture.interval_secs.max(1),
            // 空闲降频：取间隔的 4 倍，但不小于 30 秒（用户离开后仍保留粗粒度记录）
            idle_interval_secs: (capture.interval_secs.saturating_mul(4)).max(30),
            idle_threshold_secs: capture.idle_threshold_secs,
            queue_capacity: capture.capture_queue_capacity.max(1),
            max_parallel_targets: capture.max_parallel_targets.max(1),
        },
        hash: mc_capture::change::HashPolicy {
            hamming_threshold: capture.phash_hamming_threshold,
            ..mc_capture::change::HashPolicy::default()
        },
        // 隐私黑名单来自配置：写了就要拦（传空列表等于配置形同虚设）
        blocked_apps: config.privacy.blocked_apps.clone(),
        blocked_window_patterns: config.privacy.blocked_window_patterns.clone(),
        blocked_domains: mc_pipeline::domains::DomainRules::new(&config.privacy.blocked_domains),
        selected_targets: capture.target_ids.clone(),
        region: capture
            .region
            .and_then(mc_capture::geometry::Rect::from_array),
    }
}

/// 每 tick 的外界信号来源。
///
/// 抽成可注入的一层：调度器会按 `idle_for_secs` 把节奏降到 30 秒一次，
/// 而开发机有没有输入不受测试控制 —— 直接读系统会让测试结果取决于运行环境。
pub type SignalsSource = Arc<dyn Fn() -> CaptureSignals + Send + Sync>;

/// 上一次的（锁屏, 休眠）状态。`None` 表示还没读到过 —— 首次读到不发事件，
/// 否则每次 daemon 启动都会给订阅者推一条「刚刚锁屏」的假消息。
type PowerState = (bool, bool);

/// 状态变化时把锁屏/休眠事件推给渲染层。返回是否发生了变化。
fn publish_power_state(
    state: &crate::state::ServerState,
    last: &mut Option<PowerState>,
    locked: bool,
    suspended: bool,
) -> bool {
    let current = (locked, suspended);
    match *last {
        Some(previous) if previous == current => false,
        Some((was_locked, was_suspended)) => {
            *last = Some(current);
            let event_key = if locked != was_locked {
                if locked {
                    crate::events::POWER_MONITOR_LOCK_SCREEN
                } else {
                    crate::events::POWER_MONITOR_UNLOCK_SCREEN
                }
            } else if suspended {
                crate::events::POWER_MONITOR_SUSPEND
            } else {
                crate::events::POWER_MONITOR_RESUME
            };
            // 载荷是渲染层的契约：`eventKey` 决定它走哪个分支。
            state.publish(
                crate::events::EVENT_PUSH_POWER_MONITOR,
                serde_json::json!({ "eventKey": event_key, "data": { "locked": locked, "suspended": suspended } }),
            );
            let _ = was_suspended;
            true
        }
        None => {
            *last = Some(current);
            false
        }
    }
}

/// 生产实现：锁屏与空闲都从系统读；读不出来按「可以采」处理。
pub fn system_signals() -> CaptureSignals {
    CaptureSignals {
        locked: mc_common::session::screen_locked() == Some(true),
        suspended: false,
        idle_for_secs: mc_common::idle::idle_secs().unwrap_or(0),
    }
}

/// 起采集循环：没有挂载采集控制（只读实例）时立即返回，调用方不必判断。
pub fn spawn_capture_loop(state: Arc<ServerState>, tick: Duration) -> tokio::task::JoinHandle<()> {
    spawn_capture_loop_with_signals(state, tick, Arc::new(system_signals))
}

/// 与 [`spawn_capture_loop`] 相同，只是信号来源可注入（测试用固定值）。
pub fn spawn_capture_loop_with_signals(
    state: Arc<ServerState>,
    tick: Duration,
    signals: SignalsSource,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(controls) = state.capture.clone() else {
            return;
        };

        let mut pump = match build_pump(&state, &controls) {
            Ok(pump) => pump,
            Err(error) => {
                let at = Clock::now(&SystemClock);
                error!(
                    component = "capture",
                    event = "pump_build_failed",
                    detail = %error_summary(&error),
                    "采集环构建失败，不启动采集"
                );
                record_capture_failure(&state.db, at, &error);
                return;
            }
        };
        let mut current_policy = policy_from(&state.config.current().config);
        // 采集一直失败时（例如没有屏幕录制权限）不要把诊断表刷爆：
        // 失败记录按时间节流，每分钟最多一条。
        let mut last_failure_recorded: Option<Timestamp> = None;
        // 磁盘余量按分钟检查（`df` 不必每 tick 跑），低就跳过采集并写可见失败
        let mut disk_checked_at: Option<Timestamp> = None;
        let mut disk_low = false;
        // 剪贴板源是否启用：每轮从配置重算（见循环内赋值），因此不预设初值；
        // last_clipboard 记住上一条原文用于去重
        let mut clipboard_enabled;
        let mut last_clipboard: Option<String> = None;
        // 上一次的锁屏/休眠状态：只在变化时推事件（见 publish_power_state）
        let mut last_power: Option<PowerState> = None;

        loop {
            tokio::time::sleep(tick).await;

            // 配置可能在运行中变了（用户在设置里改了频率 / 选择）。
            // 用「策略指纹」比较，变了就热更新 —— 不重建 pump，待写队列不丢。
            let config = state.config.current().config.clone();
            // 剪贴板源是否启用取决于配置里的源列表（默认只有 screen/window）
            clipboard_enabled = config
                .capture
                .sources
                .iter()
                .any(|source| source == "clipboard");
            let next_policy = policy_from(&config);
            if policy_fingerprint(&next_policy) != policy_fingerprint(&current_policy) {
                if let Err(error) = pump.set_policy(next_policy.clone()) {
                    let at = Clock::now(&SystemClock);
                    record_capture_failure(&state.db, at, &error);
                } else {
                    current_policy = next_policy;
                }
            }

            if !controls.is_running() {
                continue;
            }

            let at = Clock::now(&SystemClock);

            // 录制时段：用户设了「只在工作日 08:00–20:00 采集」就必须真的生效。
            // 这是最容易做成「接口收下了、行为没变」的地方，因此放在 tick 之前。
            if !recording_window_allows(&config, at, &timezone_of(&state)) {
                debug!(
                    component = "capture",
                    event = "window_blocked",
                    "当前不在录制时段"
                );
                continue;
            }

            // 锁屏就先不采：锁屏画面属于隐私内容，录进去等于把用户离开后的桌面记下来。
            // 探测读不出来时按「未锁定」处理 —— 宁可多采，也不能因探测失败整体停采。
            let signals = signals();
            // 状态**变化**时通知渲染层：它订阅了 push:power-monitor 用来暂停/恢复轮询。
            // 只在变化时发，避免每个 tick 都推一条没人要的心跳。
            publish_power_state(&state, &mut last_power, signals.locked, signals.suspended);
            if signals.locked {
                if should_record_failure(last_failure_recorded, at) {
                    let error = AppError::new(
                        mc_common::error::ErrorCode::CaptureUnsupported,
                        "屏幕已锁定，暂停采集（解锁后自动继续）",
                    );
                    record_capture_failure(&state.db, at, &error);
                    last_failure_recorded = Some(at);
                }
                continue;
            }

            // 磁盘将满就先停采：写到一半失败会留下半张图与一条坏观测，
            // 而「停采 + 一条可见失败」用户能看懂、也能自己去清空间。
            if disk_checked_at.is_none_or(|checked| at.saturating_diff_millis(checked) >= 60_000) {
                disk_low = mc_common::disk::free_bytes(&state.data_dir).is_some_and(|free| {
                    mc_common::disk::should_pause(free, mc_common::disk::MIN_FREE_BYTES)
                });
                disk_checked_at = Some(at);
            }
            if disk_low {
                if should_record_failure(last_failure_recorded, at) {
                    let error = AppError::new(
                        mc_common::error::ErrorCode::StorageDiskFull,
                        format!(
                            "磁盘可用空间低于 {} MiB，已暂停采集",
                            mc_common::disk::MIN_FREE_BYTES / (1024 * 1024)
                        ),
                    );
                    record_capture_failure(&state.db, at, &error);
                    last_failure_recorded = Some(at);
                }
                warn!(
                    component = "capture",
                    event = "disk_low",
                    "磁盘可用空间不足，暂停采集"
                );
                continue;
            }
            // 剪贴板源：只在配置里显式点名 `clipboard` 时才读（fail-closed）。
            // 放在磁盘/锁屏门控之后 —— 暂停采集时剪贴板同样不该继续记。
            //
            // 落库前必须过脱敏：剪贴板是最容易夹带密钥与个人信息的入口。
            // 脱敏规则本身非法时**不落库**（与总结路径同一原则：静默存原文等于
            // 用户以为自己脱敏了、实际没有）。
            if clipboard_enabled {
                if let Some(text) = mc_capture::clipboard::read_text() {
                    if mc_capture::clipboard::is_new(last_clipboard.as_deref(), &text) {
                        match mc_common::redact::Redactor::new(&config.privacy.redact_patterns) {
                            Err(_) => {
                                if should_record_failure(last_failure_recorded, at) {
                                    let error = AppError::new(
                                        mc_common::error::ErrorCode::CaptureUnsupported,
                                        "脱敏规则非法，剪贴板内容未存储",
                                    );
                                    record_capture_failure(&state.db, at, &error);
                                    last_failure_recorded = Some(at);
                                }
                            }
                            Ok(redactor) => {
                                let redaction = redactor.redact(&text);
                                let hash = mc_capture::clipboard::content_hash(&redaction.text);
                                let verdict = if redaction.is_clean() {
                                    "allowed"
                                } else {
                                    "redacted"
                                };
                                let observation = NewObservation {
                                    id: format!("clipboard-{hash:016x}"),
                                    ts: at,
                                    source_id: "clipboard".to_string(),
                                    kind: "clipboard".to_string(),
                                    app_name: None,
                                    app_bundle_id: None,
                                    window_title: None,
                                    domain: None,
                                    display_id: None,
                                    scale_factor: None,
                                    image: None,
                                    text_content: Some(redaction.text.clone()),
                                    text_origin: Some("clipboard".to_string()),
                                    change_kind: "new".to_string(),
                                    privacy_verdict: verdict.to_string(),
                                    phash: Some(hash),
                                    idempotency: format!("clipboard:{hash:016x}"),
                                };
                                if let Err(error) = state.db.insert_observation(&observation) {
                                    warn!(
                                        component = "capture",
                                        event = "clipboard_insert_failed",
                                        message = %error_summary(&error),
                                        "剪贴板观测写入失败"
                                    );
                                }
                                // 去重按原文：原文没变就不必再走一遍脱敏与落库
                                last_clipboard = Some(text);
                            }
                        }
                    }
                }
            }

            // 空闲时长交给调度器决定是否降频（策略里有 idle_threshold_secs 与
            // idle_interval_secs）；锁屏已在上面 continue 掉，因此这里 folded 进来的是同一份信号。
            let before = pump.stats().failed;
            if let Err(error) = pump.tick(at, &signals).await {
                // 失败可见（诊断页），但循环**绝不退出**：
                // 一次采集失败不该让录制永久停摆。
                record_capture_failure(&state.db, at, &error);
            }

            // 泵内部把「采集源报错」记成 failed 并继续（它不该因为一帧失败
            // 就中断整轮），因此这里补一条可见的失败记录 ——
            // 不补的话，采集一直失败而诊断页上什么都没有。
            // 统计写进共享状态：诊断页要看的是「采集到底做了什么」，
            // 而不是「采集应该在工作」。
            let pump_stats = pump.stats();
            state.record_capture_stats(crate::state::CaptureStatsSnapshot {
                persisted: pump_stats.persisted,
                unchanged: pump_stats.unchanged,
                throttled: pump_stats.throttled,
                failed: pump_stats.failed,
                privacy_blocked: pump_stats.privacy_blocked,
                dropped: pump_stats.dropped,
            });

            debug!(
                component = "capture",
                event = "cycle",
                persisted = pump_stats.persisted,
                unchanged = pump_stats.unchanged,
                throttled = pump_stats.throttled,
                privacy_blocked = pump_stats.privacy_blocked,
                dropped = pump_stats.dropped,
                "采集轮询完成"
            );

            let failed = pump_stats.failed;
            if failed > before {
                warn!(
                    component = "capture",
                    event = "frame_failed",
                    total = failed,
                    "采集失败"
                );
            }
            if failed > before && should_record_failure(last_failure_recorded, at) {
                let error = AppError::new(
                    mc_common::error::ErrorCode::CaptureIo,
                    format!("采集失败（累计 {failed} 次，最近一次：{at:?}）"),
                );
                record_capture_failure(&state.db, at, &error);
                last_failure_recorded = Some(at);
            }
        }
    })
}

fn build_pump(
    state: &ServerState,
    controls: &Arc<crate::capture::CaptureControls>,
) -> Result<CapturePump, AppError> {
    let policy = policy_from(&state.config.current().config);
    CapturePump::new(
        Arc::clone(&controls.source),
        Arc::new(BlobFrameStore::new(Arc::clone(&controls.blobs), None)),
        Arc::new(DatabaseSink::new(Arc::clone(&state.db))),
        policy,
    )
}

/// 采集统计（供诊断页与 `/api/capture/status` 展示）。
pub fn pump_stats_label(stats: &mc_pipeline::pump::PumpStats) -> String {
    format!(
        "persisted={} unchanged={} throttled={} failed={} blocked={} dropped={}",
        stats.persisted,
        stats.unchanged,
        stats.throttled,
        stats.failed,
        stats.privacy_blocked,
        stats.dropped
    )
}

/// 失败记录的节流：同一类失败每分钟最多记一条。
///
/// 没有权限时采集会**持续**失败，逐次记录会把 `pipeline_failures` 刷爆，
/// 真正重要的失败反而被淹没。
pub fn should_record_failure(last: Option<Timestamp>, now: Timestamp) -> bool {
    const MIN_GAP_MS: i64 = 60_000;
    match last {
        None => true,
        Some(previous) => now.saturating_diff_millis(previous) >= MIN_GAP_MS,
    }
}

/// 当前配置下的录制时段。`None` = 不限制。
pub fn recording_window(config: &mc_config::Config) -> Option<mc_pipeline::window::CaptureWindow> {
    let capture = &config.capture;
    if !capture.enable_recording_hours {
        return None;
    }
    let hours = capture.recording_hours.as_ref()?;
    mc_pipeline::window::CaptureWindow::parse(
        capture.apply_to_days == mc_config::ApplyToDays::Weekday,
        &hours[0],
        &hours[1],
    )
}

fn recording_window_allows(config: &mc_config::Config, at: Timestamp, timezone: &str) -> bool {
    match recording_window(config) {
        Some(window) => window.decide_at(at, timezone).is_allowed(),
        None => true,
    }
}

fn timezone_of(state: &ServerState) -> String {
    state
        .config
        .current()
        .config
        .general
        .timezone
        .clone()
        .unwrap_or_else(|| "UTC".to_string())
}

/// 影响采集行为的配置指纹。只包含会改变策略的字段。
fn policy_fingerprint(policy: &PumpPolicy) -> String {
    format!(
        "{}|{}|{}|{}",
        policy.capture.interval_secs,
        policy.capture.idle_interval_secs,
        policy.capture.idle_threshold_secs,
        policy.selected_targets.join(","),
    )
}
