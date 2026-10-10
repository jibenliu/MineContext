//! 控制面共享状态。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mc_common::error::AppError;
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_config::ConfigHandle;
use mc_storage::Database;

use crate::capture::CaptureControls;
use crate::events::EventBus;
use crate::jobs::AdhocJobs;
use crate::routes::agent_chat::ChatStreams;

/// 向量索引因上游拒绝（401/429 等）而主动暂停时的可行动原因。
///
/// 与 `analysis_blocker`（采到但未分析）互补：分析可能已通，索引仍会静默停住。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingPause {
    pub code: String,
    pub message: String,
    pub component: String,
}

pub struct ServerState {
    pub config: ConfigHandle,
    pub db: Arc<Database>,
    pub token: String,
    pub started_at: Timestamp,
    pub data_dir: PathBuf,
    pub version: &'static str,
    /// 采集控制。只读实例（例如只跑检索）可以为 None，
    /// 此时采集相关接口如实报告「不可用」而不是 500。
    pub capture: Option<Arc<CaptureControls>>,
    /// 进程内事件总线：总结/活动产生后推给 SSE 订阅者
    pub events: EventBus,
    /// 异步总结作业登记表（任意时段总结）
    pub jobs: AdhocJobs,
    /// 对话流的中断标志
    pub chat_streams: ChatStreams,
    /// 已组装好的 embedding Provider（按配置组装一次后缓存）。
    ///
    /// 缓存而不是每次查询都新建：`reqwest::Client` 带着连接池，
    /// 每次请求重建等于放弃连接复用。daemon 的索引任务组装后写进来，
    /// 对话检索直接复用同一个实例。
    embedding: std::sync::Mutex<Option<Arc<dyn mc_providers::EmbeddingProvider>>>,
    /// 首页「最新活动」推送任务。
    pub(crate) latest_activity_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 最近一次保留策略轮转的结果（诊断页展示）。
    retention: std::sync::Mutex<Option<crate::retention::RetentionSnapshot>>,
    /// 语义索引缓存：(向量表版本戳, 索引)。
    ///
    /// 每次对话都重建索引是 O(文档数 + 向量数) 的纯 CPU 开销，而向量只在索引任务
    /// 跑完时变化 —— 用版本戳做失效键，既不陈旧也不每次重建。
    vector_index_cache: std::sync::Mutex<Option<(String, Arc<mc_search::VectorIndex>)>>,
    /// 最近一次采集轮的统计（诊断页展示「采集到底做了什么」）。
    capture_stats: std::sync::Mutex<Option<CaptureStatsSnapshot>>,
    /// 可写配置：原始分层请求 + 用户配置文件路径。
    ///
    /// 只读实例（例如只跑检索）没有这一项，此时设置类接口会给出结构化错误，
    /// 而不是假装保存成功。
    config_write: Option<(mc_config::load::LoadRequest, std::path::PathBuf)>,
    /// 链接上传用的 HTTP 传输（测试注入 ScriptedTransport；生产默认自建客户端）。
    link_transport: std::sync::Mutex<Option<Arc<dyn mc_providers::transport::HttpTransport>>>,
    /// 向量索引暂停原因（401/429 等）；`None` = 未暂停。
    ///
    /// 给 `/api/health` 的 embedding.status、托盘「索引已暂停」、设置/首页横幅与
    /// `/api/indexing/*` 共用；业务暂停逻辑在 embedding worker 的 `WorkerRetry`。
    indexing_pause: std::sync::Mutex<Option<IndexingPause>>,
    /// 一键恢复代数：worker 用它在不改配置时也能清暂停并重建 Provider。
    indexing_resume_epoch: AtomicU64,
}

impl ServerState {
    pub fn new(
        config: ConfigHandle,
        db: Arc<Database>,
        token: String,
        started_at: Timestamp,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            db,
            token,
            started_at,
            data_dir,
            version: env!("CARGO_PKG_VERSION"),
            capture: None,
            events: EventBus::new(),
            jobs: AdhocJobs::new(),
            chat_streams: ChatStreams::new(),
            embedding: std::sync::Mutex::new(None),
            latest_activity_task: std::sync::Mutex::new(None),
            retention: std::sync::Mutex::new(None),
            vector_index_cache: std::sync::Mutex::new(None),
            capture_stats: std::sync::Mutex::new(None),
            config_write: None,
            link_transport: std::sync::Mutex::new(None),
            indexing_pause: std::sync::Mutex::new(None),
            indexing_resume_epoch: AtomicU64::new(0),
        }
    }

    /// 发布控制面事件。没有订阅者时是空操作。
    pub fn publish(&self, kind: &str, data: serde_json::Value) {
        self.events.publish(kind, data);
    }

    /// 取语义索引：命中缓存就直接给（附版本戳校验），否则重建并缓存。
    pub fn cached_vector_index(&self) -> Result<Option<Arc<mc_search::VectorIndex>>, AppError> {
        let version = mc_storage::vectors::vectors_version(&self.db)?;
        let mut slot = self.vector_index_cache.lock().map_err(|_| {
            AppError::new(
                mc_common::error::ErrorCode::StorageUnavailable,
                "索引缓存锁中毒",
            )
        })?;

        if let Some((cached_version, index)) = slot.as_ref() {
            if *cached_version == version {
                return Ok(Some(Arc::clone(index)));
            }
        }

        let Some(index) = crate::retrieval::vector_index(&self.db)? else {
            // 库里还没有向量：清掉缓存，避免「没有向量却命中缓存」
            *slot = None;
            return Ok(None);
        };
        let index = Arc::new(index);
        *slot = Some((version, Arc::clone(&index)));
        Ok(Some(index))
    }

    pub fn with_capture(mut self, capture: Arc<CaptureControls>) -> Self {
        self.capture = Some(capture);
        self
    }

    /// 声明用户配置可写：设置类接口据此把改动落到这个文件并热重载。
    pub fn with_config_write(
        mut self,
        request: mc_config::load::LoadRequest,
        user_config: std::path::PathBuf,
    ) -> Self {
        self.config_write = Some((request, user_config));
        self
    }

    /// 注入链接抓取传输（业务测试用脚本化替身，避免真出网）。
    pub fn with_link_transport(
        self,
        transport: Arc<dyn mc_providers::transport::HttpTransport>,
    ) -> Self {
        if let Ok(mut slot) = self.link_transport.lock() {
            *slot = Some(transport);
        }
        self
    }

    pub fn link_transport(&self) -> Option<Arc<dyn mc_providers::transport::HttpTransport>> {
        self.link_transport
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// 可写配置（请求 + 路径）。`None` = 本实例不可写配置。
    pub fn config_write(&self) -> Option<&(mc_config::load::LoadRequest, std::path::PathBuf)> {
        self.config_write.as_ref()
    }

    /// 注入 embedding Provider（daemon 索引任务与测试共用）。
    pub fn set_embedding_provider(&self, provider: Arc<dyn mc_providers::EmbeddingProvider>) {
        if let Ok(mut slot) = self.embedding.lock() {
            *slot = Some(provider);
        }
    }

    pub fn embedding_provider(&self) -> Option<Arc<dyn mc_providers::EmbeddingProvider>> {
        self.embedding.lock().ok().and_then(|slot| slot.clone())
    }

    pub(crate) fn record_capture_stats(&self, snapshot: CaptureStatsSnapshot) {
        if let Ok(mut slot) = self.capture_stats.lock() {
            *slot = Some(snapshot);
        }
    }

    pub fn capture_stats(&self) -> Option<CaptureStatsSnapshot> {
        self.capture_stats.lock().ok().and_then(|slot| *slot)
    }

    pub(crate) fn record_retention_run(&self, snapshot: crate::retention::RetentionSnapshot) {
        if let Ok(mut slot) = self.retention.lock() {
            *slot = Some(snapshot);
        }
    }

    /// 最近一次轮转快照；从未执行过时为 `None`。
    pub fn last_retention_run(&self) -> Option<crate::retention::RetentionSnapshot> {
        self.retention.lock().ok().and_then(|slot| *slot)
    }

    /// 清掉缓存的 Provider（配置变了 / 维度不一致需要重新组装时）。
    pub fn clear_embedding_provider(&self) {
        if let Ok(mut slot) = self.embedding.lock() {
            *slot = None;
        }
    }

    pub fn set_indexing_pause(&self, pause: IndexingPause) {
        if let Ok(mut slot) = self.indexing_pause.lock() {
            *slot = Some(pause);
        }
    }

    pub fn clear_indexing_pause(&self) {
        if let Ok(mut slot) = self.indexing_pause.lock() {
            *slot = None;
        }
    }

    pub fn indexing_pause(&self) -> Option<IndexingPause> {
        self.indexing_pause
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    pub fn indexing_resume_epoch(&self) -> u64 {
        self.indexing_resume_epoch.load(Ordering::SeqCst)
    }

    /// 一键恢复索引：清暂停、丢弃缓存 Provider、递增代数让 worker 立刻重试。
    pub fn request_indexing_resume(&self) {
        self.clear_indexing_pause();
        self.clear_embedding_provider();
        self.indexing_resume_epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// 托盘 / `/api/health` 兼容入口：有任意索引暂停原因即视为 paused。
    pub fn embedding_auth_paused(&self) -> bool {
        self.indexing_pause().is_some()
    }

    /// 测试与旧调用点：写入/清除一条鉴权类暂停原因。
    pub fn set_embedding_auth_paused(&self, paused: bool) {
        if paused {
            self.set_indexing_pause(IndexingPause {
                code: "api_key_invalid".to_string(),
                message: "向量索引已暂停：API Key 无效或已过期。请到设置页更新密钥后点「恢复索引」。"
                    .to_string(),
                component: "embedding".to_string(),
            });
        } else {
            self.clear_indexing_pause();
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        let elapsed = SystemClock.now().saturating_diff_millis(self.started_at);
        if elapsed <= 0 {
            0
        } else {
            (elapsed / 1000) as u64
        }
    }
}

/// 采集统计快照。`dropped` 必须恒为 0 —— 它是「有没有丢观测」的哨兵。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureStatsSnapshot {
    pub persisted: u64,
    pub unchanged: u64,
    pub throttled: u64,
    pub failed: u64,
    pub privacy_blocked: u64,
    pub dropped: u64,
}
