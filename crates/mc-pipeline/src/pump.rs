//! 采集环：把调度、采集、变化检测、隐私过滤、落盘、落库串成一条流水线。
//!
//! 这一层也是「采集与智能解耦」的落点：**采集只负责产出观测，绝不等待分析**。
//!
//! 三条不变量：
//! 1. **过载时降频，绝不丢观测**：写入失败时观测留在待写队列，队列满则降频。
//! 2. **没变化的帧不落盘也不落库**：这是「不花冤枉钱」的关键。
//! 3. **被隐私规则拦截的内容绝不落盘**：但保留一条可审计的元数据。

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use image::RgbImage;
use mc_capture::change::{
    rgb_to_luma, ChangeDetector, ChangeKind, DHashDetector, FrameStats, HashPolicy,
};
use mc_capture::geometry::Rect;
use mc_capture::scheduler::{CapturePolicy, CaptureScheduler, CaptureSignals, TickOutcome};
use mc_capture::source::{CaptureSource, CaptureTarget, SourceKind, TargetKind};

/// 区域采集的矩形。
pub type RegionRect = Rect;
use mc_common::error::AppError;
use mc_common::observability::debug;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyVerdict {
    Allowed,
    Redacted,
    Blocked,
}

/// 已落盘的帧（路径与哈希，不含二进制）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFrame {
    pub relative_path: String,
    pub content_hash: String,
    pub thumbnail_path: Option<String>,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

/// 帧存储。实现方是 `mc_storage::blob::FileSystemBlobStore` 的适配器。
pub trait FrameStore: Send + Sync {
    fn store_frame(&self, image: &RgbImage, at: Timestamp) -> Result<StoredFrame, AppError>;
}

/// 一条可以落库的观测。
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedObservation {
    pub id: String,
    pub captured_at: Timestamp,
    pub target: CaptureTarget,
    pub source_id: String,
    pub source_kind: SourceKind,
    pub change_kind: ChangeKind,
    pub privacy_verdict: PrivacyVerdict,
    pub image: Option<StoredFrame>,
    pub phash: u64,
    /// 幂等键：`{source_id}|{target_id}|{content_hash}`。
    /// 可读优先于定长 —— 出问题时能一眼看出是哪一帧。
    pub idempotency: String,
}

/// 观测去向。实现方是 `mc-storage` 的 `insert_observation` 适配器。
pub trait ObservationSink: Send + Sync {
    fn persist(&self, observation: &CapturedObservation) -> Result<(), AppError>;
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PumpPolicy {
    pub capture: CapturePolicy,
    pub hash: HashPolicy,
    pub blocked_apps: Vec<String>,
    pub blocked_window_patterns: Vec<String>,
    /// 域名黑名单（`privacy.blocked_domains`）。
    pub blocked_domains: crate::domains::DomainRules,
    /// 要采集的目标 id 白名单。**空 = 全部可见目标**。
    ///
    /// 之所以「空 = 全部」而不是「空 = 不采」：选择只存在内存里时，
    /// 重启后白名单为空会让采集静默停摆。
    /// 用户要排除某块屏就显式写出来；没写就是没限制。
    pub selected_targets: Vec<String>,
    /// 只采集这块区域（`None` = 逐个显示器整屏采集）。
    ///
    /// 设了它就**只**截这一块，而不是「额外再截一块」。
    pub region: Option<RegionRect>,
}

impl PumpPolicy {
    /// 某个目标是否应当被采集：可见 + 在白名单内（白名单为空时不筛）
    /// + 区域模式下与区域有重叠。
    pub fn accepts(&self, target: &CaptureTarget) -> bool {
        if !target.is_visible {
            return false;
        }
        if !self.selected_targets.is_empty() && !self.selected_targets.contains(&target.id) {
            return false;
        }
        match self.region {
            // 没有几何信息时无法判定重叠，只能放行（由裁剪阶段降级为整屏）
            None => true,
            Some(region) => match target.bounds {
                Some(bounds) => bounds.intersect(region).is_some(),
                None => true,
            },
        }
    }

    /// 区域模式下该目标要截的局部像素范围 `(x, y, w, h)`。
    ///
    /// `None` = 不裁剪（整屏）。源没有几何信息时也返回 `None`：
    /// 拿不到坐标系就无法裁剪，此时整屏采集是明确的降级行为。
    pub fn crop_for(&self, target: &CaptureTarget) -> Option<(u32, u32, u32, u32)> {
        let region = self.region?;
        let bounds = target.bounds?;
        bounds.intersect(region)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PumpStats {
    pub persisted: u64,
    /// 画面没变，因此没有落盘也没有落库
    pub unchanged: u64,
    pub throttled: u64,
    pub failed: u64,
    pub privacy_blocked: u64,
    /// 哨兵：必须恒为 0。非 0 说明有观测被丢弃（不允许发生）。
    pub dropped: u64,
}

pub struct CapturePump {
    source: Arc<dyn CaptureSource>,
    frames: Arc<dyn FrameStore>,
    sink: Arc<dyn ObservationSink>,
    scheduler: CaptureScheduler,
    detector: DHashDetector,
    policy: PumpPolicy,
    previous: BTreeMap<String, FrameStats>,
    /// 上一次记录过的窗口元数据（应用名, 标题, 是否被拦截）。
    /// 用来避免「标题没变也每轮写一行」—— 15s 一条会很快淹掉时间线。
    last_metadata: BTreeMap<String, (Option<String>, Option<String>, bool)>,
    pending: VecDeque<CapturedObservation>,
    stats: PumpStats,
    last_error: Option<AppError>,
    targets_loaded: bool,
}

impl CapturePump {
    pub fn new(
        source: Arc<dyn CaptureSource>,
        frames: Arc<dyn FrameStore>,
        sink: Arc<dyn ObservationSink>,
        policy: PumpPolicy,
    ) -> Result<Self, AppError> {
        let detector = DHashDetector::new(policy.hash.clone())?;
        Ok(Self {
            source,
            frames,
            sink,
            scheduler: CaptureScheduler::new(policy.capture.clone()),
            detector,
            policy,
            previous: BTreeMap::new(),
            last_metadata: BTreeMap::new(),
            pending: VecDeque::new(),
            stats: PumpStats::default(),
            last_error: None,
            targets_loaded: false,
        })
    }

    /// 换一套策略（用户在设置里改了间隔/选择/阈值）。
    ///
    /// 只重建调度器与变化检测、并让下一次 tick 重新枚举目标；
    /// **保留**待写队列与累计统计 —— 那些是运行状态，不该因为改设置丢掉。
    pub fn set_policy(&mut self, policy: PumpPolicy) -> Result<(), AppError> {
        self.detector = DHashDetector::new(policy.hash.clone())?;
        self.scheduler = CaptureScheduler::new(policy.capture.clone());
        self.previous.clear();
        self.targets_loaded = false;
        self.policy = policy;
        Ok(())
    }

    pub fn stats(&self) -> PumpStats {
        self.stats
    }

    /// 待写条数。与调度器的队列占用保持一致。
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn last_error(&self) -> Option<&AppError> {
        self.last_error.as_ref()
    }

    pub fn scheduler(&self) -> &CaptureScheduler {
        &self.scheduler
    }

    pub fn scheduler_mut(&mut self) -> &mut CaptureScheduler {
        &mut self.scheduler
    }

    /// 从采集源重新枚举可采集目标。
    ///
    /// 由 `tick` 在首次运行时自动调用一次 —— `enumerate` 可能要枚举系统窗口，
    /// 不该每帧都做。显示器热插拔后由调用方主动触发。
    pub async fn refresh_targets(&mut self) -> Result<usize, AppError> {
        let targets = self.source.enumerate().await?;
        // 只把「可见且被选中」的目标交给调度器：未选中的显示器不该占用队列名额，
        // 也不该让调度器为它们记时间戳。
        let ids: Vec<String> = targets
            .into_iter()
            .filter(|target| self.policy.accepts(target))
            .map(|t| t.id)
            .collect();
        let count = ids.len();
        self.scheduler.set_targets(ids);
        self.targets_loaded = true;
        Ok(count)
    }

    /// 驱动一轮。
    ///
    /// 顺序很重要：**先把上一轮没写成功的补上**，再考虑抓新帧 ——
    /// 否则一次数据库抖动就会让观测永久丢失。
    pub async fn tick(
        &mut self,
        now: Timestamp,
        signals: &CaptureSignals,
    ) -> Result<TickOutcome, AppError> {
        self.flush_pending();

        if !self.targets_loaded {
            // 枚举失败不致命：可能是权限问题，本轮没有目标即可，
            // 下一轮会再试（因此只在成功时置位）
            let _ = self.refresh_targets().await;
        }

        let outcome = self.scheduler.tick(now, signals);
        if outcome.throttled {
            self.stats.throttled += 1;
        }
        if outcome.started.is_empty() {
            return Ok(outcome);
        }

        let ctx = mc_capture::source::CaptureContext {
            targets: outcome.started.clone(),
            now,
            idle: signals.idle_for_secs > 0,
        };

        let captures = match self.source.poll(&ctx).await {
            Ok(captures) => captures,
            Err(error) => {
                self.stats.failed += 1;
                debug!(
                    component = "capture",
                    event = "frame_error",
                    detail = %mc_common::observability::error_summary(&error),
                    "这一轮采集失败"
                );
                self.last_error = Some(error.clone());
                for id in &outcome.started {
                    self.scheduler.mark_captured(id);
                }
                // 采集失败不中断循环，只是本轮没有产出
                return Ok(outcome);
            }
        };

        // 前台窗口命中黑名单时，**同一轮的屏幕帧也要一起拦**：
        // 屏幕截图里同样是那个应用的内容，只拦窗口观测等于黑名单形同虚设。
        // 先把整轮看一遍再逐条处理 —— 两个源的返回顺序不保证屏幕在前。
        let screen_veto = captures.iter().any(|capture| {
            capture.target.kind == TargetKind::Window
                && self.evaluate_privacy(&capture.target) == PrivacyVerdict::Blocked
        });

        let mut seen: Vec<String> = Vec::new();
        for capture in captures {
            seen.push(capture.target.id.clone());
            self.handle_capture(capture, now, screen_veto);
        }

        // 没有产出的目标也要释放「在途」标记，否则会被永久卡住
        for id in &outcome.started {
            self.scheduler.mark_captured(id);
        }

        Ok(outcome)
    }

    fn handle_capture(
        &mut self,
        capture: mc_capture::source::RawCapture,
        now: Timestamp,
        screen_veto: bool,
    ) {
        let Some(image) = capture.image.clone() else {
            // 没有图像的源（窗口元数据、剪贴板等）不参与像素比较，
            // 但**必须落成观测**：应用名与标题是隐私判定与活动识别的输入。
            self.handle_metadata_capture(capture, now, screen_veto);
            return;
        };

        // 区域模式：先裁到区域，再做变化检测与落盘 ——
        // 否则「区域外的变化」会一直被判定为「有意义的变化」而落盘，
        // 用户看到的截图内容却完全一样。
        let image = match self.policy.crop_for(&capture.target) {
            Some((x, y, width, height)) => crop(&image, x, y, width, height),
            None => Some(image),
        };
        let Some(image) = image else {
            // 区域与帧完全没有重叠（显示器刚被拔掉之类）：丢掉这一帧而不是存空图
            self.stats.failed += 1;
            self.scheduler.drain_pending(1);
            return;
        };

        let luma = rgb_to_luma(&image);
        let stats = self.detector.analyze(&luma);
        let previous = self.previous.get(&capture.target.id);
        let change_kind = self.detector.classify(previous, &stats, false);
        self.previous.insert(capture.target.id.clone(), stats);

        if !change_kind.needs_analysis() {
            self.stats.unchanged += 1;
            self.scheduler.drain_pending(1);
            return;
        }

        let verdict = if screen_veto && capture.target.kind == TargetKind::Screen {
            PrivacyVerdict::Blocked
        } else {
            self.evaluate_privacy(&capture.target)
        };

        let stored = if verdict == PrivacyVerdict::Blocked {
            None
        } else {
            match self.frames.store_frame(&image, now) {
                Ok(frame) => Some(frame),
                Err(error) => {
                    // 不 drain：观测留在队列里等下一轮重试
                    self.stats.failed += 1;
                    self.last_error = Some(error);
                    return;
                }
            }
        };

        if verdict == PrivacyVerdict::Blocked {
            self.stats.privacy_blocked += 1;
        }

        let mut target = capture.target.clone();
        if verdict == PrivacyVerdict::Blocked {
            // 审计只留应用名，不留可能含敏感内容的窗口标题
            target.window_title = None;
        }

        let content_hash = stored
            .as_ref()
            .map(|f| f.content_hash.clone())
            .unwrap_or_else(|| {
                blake3::hash(format!("{}|{}", capture.target.id, now.as_millis()).as_bytes())
                    .to_hex()
                    .to_string()
            });

        let observation = CapturedObservation {
            id: uuid::Uuid::now_v7().to_string(),
            captured_at: now,
            target,
            // 来源取自**这一帧**（多源合并时不能都记成组合源）
            source_id: capture.source_id.clone(),
            source_kind: capture.source_kind,
            change_kind,
            privacy_verdict: verdict,
            image: stored,
            phash: stats.phash,
            idempotency: format!(
                "{}|{}|{}",
                capture.source_id, capture.target.id, content_hash
            ),
        };

        self.pending.push_back(observation);
        self.flush_pending();
    }

    /// 把待写队列尽量写空。失败就停下，留待下一轮 —— 绝不丢弃。
    fn flush_pending(&mut self) {
        while let Some(observation) = self.pending.front() {
            match self.sink.persist(observation) {
                Ok(()) => {
                    self.pending.pop_front();
                    self.scheduler.drain_pending(1);
                    self.stats.persisted += 1;
                }
                Err(error) => {
                    self.stats.failed += 1;
                    self.last_error = Some(error);
                    break;
                }
            }
        }
    }

    /// 只有元数据（没有图像）的观测：窗口的应用名 + 标题。
    ///
    /// 三条规则：
    /// - 命中黑名单 → 仍然留一行审计（应用名 + 时间，**不留标题、不留图像**）；
    /// - 元数据没变 → 不重复写（否则每 15s 一行，时间线会被同类行淹掉）；
    /// - 变了 → 写一行 `ChangeKind::TitleOnly`。
    fn handle_metadata_capture(
        &mut self,
        capture: mc_capture::source::RawCapture,
        now: Timestamp,
        screen_veto: bool,
    ) {
        let mut target = capture.target.clone();

        let verdict = if screen_veto && target.kind == TargetKind::Screen {
            PrivacyVerdict::Blocked
        } else {
            self.evaluate_privacy(&target)
        };

        if verdict == PrivacyVerdict::Blocked {
            // 审计只留应用名：标题可能本身就含敏感内容
            target.window_title = None;
            self.stats.privacy_blocked += 1;
        }

        let key = (
            target.app_name.clone(),
            target.window_title.clone(),
            verdict == PrivacyVerdict::Blocked,
        );
        if self.last_metadata.get(&target.id) == Some(&key) {
            self.stats.unchanged += 1;
            self.scheduler.drain_pending(1);
            return;
        }
        self.last_metadata.insert(target.id.clone(), key);

        let idempotency = format!(
            "{}|{}|metadata|{}",
            capture.source_id,
            target.id,
            now.as_millis()
        );

        let observation = CapturedObservation {
            id: uuid::Uuid::now_v7().to_string(),
            captured_at: now,
            target,
            // 来源取自**这一帧**（多源合并时不能都记成组合源）
            source_id: capture.source_id,
            source_kind: capture.source_kind,
            // 只有元数据变了，画面这一路没有参与
            change_kind: ChangeKind::TitleOnly,
            privacy_verdict: verdict,
            image: None,
            phash: 0,
            idempotency,
        };

        self.pending.push_back(observation);
        self.flush_pending();
        self.scheduler.drain_pending(1);
    }

    fn evaluate_privacy(&self, target: &CaptureTarget) -> PrivacyVerdict {
        let app_blocked = target
            .app_name
            .as_deref()
            .map(|app| {
                self.policy
                    .blocked_apps
                    .iter()
                    .any(|blocked| blocked.eq_ignore_ascii_case(app))
            })
            .unwrap_or(false);

        if app_blocked {
            return PrivacyVerdict::Blocked;
        }

        let title = target
            .window_title
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        let title_blocked = self
            .policy
            .blocked_window_patterns
            .iter()
            .any(|pattern| !pattern.is_empty() && title.contains(&pattern.to_lowercase()));

        if title_blocked {
            return PrivacyVerdict::Blocked;
        }

        // 域名规则：目前只有窗口标题可用（没有 URL），走标题启发式
        if self.policy.blocked_domains.is_blocked(None, &title) {
            return PrivacyVerdict::Blocked;
        }

        PrivacyVerdict::Allowed
    }
}

/// 按局部像素范围裁剪。越界时按帧边界收敛；收敛后为空返回 `None`。
fn crop(image: &RgbImage, x: u32, y: u32, width: u32, height: u32) -> Option<RgbImage> {
    let left = x.min(image.width());
    let top = y.min(image.height());
    let right = (x.saturating_add(width)).min(image.width());
    let bottom = (y.saturating_add(height)).min(image.height());
    if right <= left || bottom <= top {
        return None;
    }
    Some(image::imageops::crop_imm(image, left, top, right - left, bottom - top).to_image())
}
