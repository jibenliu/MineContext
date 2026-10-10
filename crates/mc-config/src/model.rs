//! 配置数据模型。
//!
//! 两条刻意的类型级约束：
//!
//! 1. **只有一种 Provider**：[`ProviderKind`] 只有一个变体 `OpenAiCompatible`。
//!    任何厂商特化（豆包 / Ark 的 `multimodal_embeddings` 之类）都无法通过配置
//!    进入系统——这是类型级的防线。
//! 2. **没有明文密钥字段**：只有 `api_key_ref`（指向 Keychain），
//!    配置里出现明文 key 在结构上不可能。

use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub config_version: u32,
    pub general: General,
    pub capture: Capture,
    pub ai: Ai,
    pub activity: Activity,
    pub stage: Stage,
    pub summary: Summary,
    pub privacy: Privacy,
    pub storage: Storage,
    pub observability: Observability,
    pub server: Server,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            general: General::default(),
            capture: Capture::default(),
            ai: Ai::default(),
            activity: Activity::default(),
            stage: Stage::default(),
            summary: Summary::default(),
            privacy: Privacy::default(),
            storage: Storage::default(),
            observability: Observability::default(),
            server: Server::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub locale: String,
    /// IANA 时区名。`None` = 使用系统时区（会给出 warning）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            locale: "zh-CN".to_string(),
            timezone: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capture {
    pub enabled: bool,
    pub interval_secs: u64,
    pub sources: Vec<String>,
    /// 要采集的目标 id 白名单（例如 `["display-1", "display-2"]`）。
    ///
    /// **空 = 全部可见目标**。
    ///
    /// 之所以明确规定「空 = 全部」而不是「空 = 不采」：选择只存在内存里时，
    /// 重启后为空会让采集静默停摆，而用户无从判断原因。
    #[serde(default)]
    pub target_ids: Vec<String>,
    /// 只采集这块区域 `[left, top, right, bottom]`（虚拟屏坐标）。
    ///
    /// 设了它就只截这一块，而不是「额外再截一块」。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<[i64; 4]>,
    /// 是否只在指定时段采集
    pub enable_recording_hours: bool,
    /// 采集时段 `["HH:mm:ss", "HH:mm:ss"]`，与 `enable_recording_hours` 配套
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_hours: Option<[String; 2]>,
    /// 采集时段适用于一周里的哪些天
    pub apply_to_days: ApplyToDays,
    /// 连续空闲多久算「用户离开」，之后改用 [`Self::idle_interval_secs`]。
    pub idle_threshold_secs: u64,
    /// 用户空闲时的采集间隔（秒）。省磁盘与电池；锁屏硬暂停见采集环 / 调度器。
    pub idle_interval_secs: u64,
    pub retention_days: u32,
    /// 采集池并发
    pub max_parallel_targets: usize,
    pub capture_queue_capacity: usize,
    /// 变化检测：dHash Hamming 距离阈值
    pub phash_hamming_threshold: u32,
    /// 过载合并窗口
    pub coalesce_window_secs: u64,
    /// 解码前允许的最大边长
    pub max_decoded_edge: u32,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 15,
            sources: vec!["screen".to_string(), "window".to_string()],
            target_ids: Vec::new(),
            region: None,
            enable_recording_hours: false,
            recording_hours: None,
            apply_to_days: ApplyToDays::Weekday,
            idle_threshold_secs: 300,
            idle_interval_secs: 60,
            retention_days: 7,
            max_parallel_targets: 4,
            capture_queue_capacity: 32,
            phash_hamming_threshold: 4,
            coalesce_window_secs: 60,
            max_decoded_edge: 1568,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ai {
    pub enabled: bool,
    pub vision: Endpoint,
    pub chat: Endpoint,
    pub embedding: EmbeddingEndpoint,
    pub budget: Budget,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            enabled: true,
            vision: Endpoint::default(),
            chat: Endpoint::default(),
            embedding: EmbeddingEndpoint::default(),
            budget: Budget::default(),
        }
    }
}

/// 采集时段适用的日子。取值与前端既有的 `ApplyToDays` 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyToDays {
    /// 仅工作日
    #[default]
    Weekday,
    /// 每天
    Everyday,
}

/// 唯一合法的 Provider 种类。
///
/// 显式 `rename` 而不是 `rename_all = "snake_case"`：后者会把
/// `OpenAiCompatible` 拆成 `open_ai_compatible`，而对外契约是
/// `openai_compatible`（见 `docs/api-and-frontend.md`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProviderKind {
    #[default]
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Endpoint {
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    /// Keychain 引用，形如 `keychain:provider:vision`。**绝不存明文。**
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_ref: Option<String>,
    pub timeout_secs: u64,
    /// Provider 并发闸门
    pub max_concurrency: u32,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            provider: ProviderKind::default(),
            base_url: String::new(),
            model: String::new(),
            api_key_ref: None,
            timeout_secs: 60,
            max_concurrency: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbeddingEndpoint {
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_ref: Option<String>,
    pub timeout_secs: u64,
    pub max_concurrency: u32,
    /// 仅作**参考**：真实维度一律以 Provider 返回的向量长度为准。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<usize>,
    /// 一次请求最多提交多少条文本。
    ///
    /// **必须可配**：各家端点上限不同（64 / 100 / 2048），写死就会在
    /// 某些端点上稳定 400：把一整天的内容拼成一个巨型 `input` 数组会超限。
    pub batch_limit: usize,
}

/// 默认批量上限。64 是主流 OpenAI 兼容端点的安全值。
pub const DEFAULT_EMBEDDING_BATCH_LIMIT: usize = 64;

impl Default for EmbeddingEndpoint {
    fn default() -> Self {
        Self {
            provider: ProviderKind::default(),
            base_url: String::new(),
            model: String::new(),
            api_key_ref: None,
            timeout_secs: 60,
            max_concurrency: 2,
            dimensions: None,
            batch_limit: DEFAULT_EMBEDDING_BATCH_LIMIT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budget {
    pub max_vlm_calls_per_hour: u32,
    pub max_tokens_per_hour: u64,
    pub max_tokens_per_day: u64,
    pub on_exceeded: BudgetAction,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_vlm_calls_per_hour: 240,
            max_tokens_per_hour: 400_000,
            max_tokens_per_day: 2_000_000,
            on_exceeded: BudgetAction::Degrade,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetAction {
    Degrade,
    Pause,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStrategy {
    Rules,
    Ai,
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Activity {
    pub strategy: ActivityStrategy,
    /// 防抖：短暂切走再回来不产生新活动
    pub debounce_secs: u64,
    pub min_duration_secs: u64,
    pub merge_gap_secs: u64,
    pub max_duration_secs: u64,
    /// 多久没有观测就认为活动结束
    pub idle_gap_secs: u64,
    /// 后台投影的间隔。调小 = 活动更及时，代价是更频繁的全量重算。
    pub project_tick_secs: u64,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            strategy: ActivityStrategy::Hybrid,
            debounce_secs: 45,
            min_duration_secs: 60,
            merge_gap_secs: 120,
            max_duration_secs: 7200,
            idle_gap_secs: 300,
            project_tick_secs: 30,
        }
    }
}

impl Activity {
    /// 配置 → 领域聚合策略。
    ///
    /// 放在这里而不是各调用点各写一遍：**重放必须和实时用同一套参数**，
    /// 两处各写一份的结果是「重算出来的时间线和线上不一样」，
    /// 而用户只会认为「重放坏了」。
    pub fn aggregation_policy(&self) -> mc_domain::activity::AggregationPolicy {
        mc_domain::activity::AggregationPolicy {
            debounce_secs: self.debounce_secs,
            min_duration_secs: self.min_duration_secs,
            merge_gap_secs: self.merge_gap_secs,
            max_duration_secs: self.max_duration_secs,
            idle_gap_secs: self.idle_gap_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stage {
    pub min_duration_secs: u64,
    pub max_duration_secs: u64,
    /// 活动显著变化后需持续多久才判定阶段结束
    pub switch_grace_secs: u64,
    pub idle_threshold_secs: u64,
    pub min_activity_stable_secs: u64,
    /// 阶段关闭后必须在多久内产出总结（`no_stage_without_summary` 的兜底时限）
    pub summary_deadline_secs: u64,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            min_duration_secs: 900,
            max_duration_secs: 7200,
            switch_grace_secs: 300,
            idle_threshold_secs: 300,
            min_activity_stable_secs: 60,
            summary_deadline_secs: 600,
        }
    }
}

impl Stage {
    /// 配置 → 领域阶段策略。
    ///
    /// 时区从 `general.timezone` 传进来：日边界必须按用户所在时区算，
    /// 否则「今天的工作」在跨时区或 DST 日会被切错。
    pub fn stage_policy(&self, timezone: &str) -> mc_domain::stage::StagePolicy {
        mc_domain::stage::StagePolicy {
            min_duration_secs: self.min_duration_secs,
            max_duration_secs: self.max_duration_secs,
            switch_grace_secs: self.switch_grace_secs,
            idle_threshold_secs: self.idle_threshold_secs,
            min_activity_stable_secs: self.min_activity_stable_secs,
            summary_deadline_secs: self.summary_deadline_secs,
            timezone: timezone.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Summary {
    pub template_id: String,
    /// 用户自定义模板（YAML）。给了它就以它为准，`template_id` 只用于记录与回退；
    /// 解析失败**明确报错**，不静默退回内置模板。
    pub template_yaml: Option<String>,
    /// 阶段总结默认写入笔记树；任意时段总结默认不写（用户主动行为，不应污染笔记）
    pub stage_save_to_vault: bool,
    pub adhoc_save_to_vault: bool,
    /// 超过该跨度就走分块 + 归并
    pub adhoc_chunk_threshold_secs: u64,
    pub adhoc_max_chunks: usize,
}

impl Default for Summary {
    fn default() -> Self {
        Self {
            template_id: "work_stage".to_string(),
            template_yaml: None,
            stage_save_to_vault: true,
            adhoc_save_to_vault: false,
            adhoc_chunk_threshold_secs: 4 * 3600,
            adhoc_max_chunks: 24,
        }
    }
}

/// 隐私规则（见 `docs/privacy.md`）。
///
/// `Default` 就是「最保守」：`ai_upload = false`（不出网）、规则列表全空。
/// 规则引擎加载失败时也按本默认判定为 `Blocked`（fail-closed），
/// 因此这里刻意不提供任何「默认放行」的字段值。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Privacy {
    /// 默认不出网：只有用户显式同意后，内容才可能发给模型服务
    pub ai_upload: bool,
    pub blocked_apps: Vec<String>,
    pub blocked_window_patterns: Vec<String>,
    pub blocked_domains: Vec<String>,
    pub redact_patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Storage {
    /// 截图 blob 根目录。`None` = 使用平台默认数据目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob_dir: Option<String>,
    pub max_total_gb: f64,
    pub max_screenshot_count: u64,
    pub screenshot_format: String,
    pub screenshot_quality: u8,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            blob_dir: None,
            max_total_gb: 10.0,
            max_screenshot_count: 200_000,
            screenshot_format: "webp".to_string(),
            screenshot_quality: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Observability {
    pub log_level: String,
    pub log_json: bool,
    pub metrics_enabled: bool,
}

impl Default for Observability {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            log_json: true,
            metrics_enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub host: String,
    /// `0` = 由操作系统分配随机端口
    pub port: u16,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0,
        }
    }
}
