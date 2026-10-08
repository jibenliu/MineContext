//! 观测的持久化与读取。
//!
//! 两条硬要求：
//! 1. **观测与事件同事务**：不允许出现「有事件没观测」或反过来
//! 2. **幂等**：同一份内容重复投递只留一行
//!
//! **数据库只存路径与哈希，绝不存图片二进制**。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::map_sqlite_error;

/// 观测引用的图片。只存路径与哈希，不含二进制。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub relative_path: String,
    pub content_hash: String,
    pub thumbnail_path: Option<String>,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewObservation {
    pub id: String,
    pub ts: Timestamp,
    pub source_id: String,
    /// `screen` | `window` | `clipboard` | `file` | `browser`
    pub kind: String,
    pub app_name: Option<String>,
    pub app_bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub domain: Option<String>,
    pub display_id: Option<String>,
    pub scale_factor: Option<f32>,
    pub image: Option<ImageRef>,
    pub text_content: Option<String>,
    pub text_origin: Option<String>,
    /// `new` | `title_only` | `pixel_minor` | `pixel_major` | `idle` | `unknown`
    pub change_kind: String,
    /// `allowed` | `redacted` | `blocked`
    pub privacy_verdict: String,
    pub phash: Option<u64>,
    /// 幂等键（内容哈希 + 源 + 时间桶），重复投递会被忽略
    pub idempotency: String,
}

impl NewObservation {
    /// 事件 payload。字段名与 `docs/api-and-frontend.md` 的 SSE / API 形状保持一致。
    pub fn event_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "observation_id": self.id,
            "source_id": self.source_id,
            "kind": self.kind,
            "app_name": self.app_name,
            "window_title": self.window_title,
            // domain/text 是活动规则匹配的输入（关键词规则需要正文）：
            // 事件里没有它们，规则就只能靠进程名判定了。
            "domain": self.domain,
            "text": self.text_content,
            "change_kind": self.change_kind,
            "privacy_verdict": self.privacy_verdict,
            "image_path": self.image.as_ref().map(|i| i.relative_path.clone()),
            "image_blob_hash": self.image.as_ref().map(|i| i.content_hash.clone()),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertOutcome {
    pub id: String,
    /// false = 命中幂等键，返回的是已存在那一行
    pub inserted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRow {
    pub id: String,
    pub ts: Timestamp,
    pub source_id: String,
    pub kind: String,
    pub app_name: Option<String>,
    pub app_bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub display_id: Option<String>,
    pub scale_factor: Option<f32>,
    pub image_path: Option<String>,
    pub image_blob_hash: Option<String>,
    pub thumbnail_path: Option<String>,
    pub image_bytes: Option<u64>,
    pub image_w: Option<u32>,
    pub image_h: Option<u32>,
    pub phash: Option<u64>,
    pub change_kind: String,
    pub privacy_verdict: String,
    pub analysis_status: Option<String>,
}

/// 时间线查询条件。区间是**半开** `[from, to)`，避免相邻区间重复计算。
#[derive(Debug, Clone, Default)]
pub struct ObservationQuery {
    pub from: Option<Timestamp>,
    pub to: Option<Timestamp>,
    pub app_name: Option<String>,
    /// 隐私拦截的观测默认不出现在时间线里
    pub exclude_blocked: bool,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

const DEFAULT_LIMIT: u32 = 100;

const SELECT_COLUMNS: &str = "\
    o.id, o.ts_utc_ms, o.source_id, o.kind, o.app_name, o.app_bundle_id, o.window_title, \
    o.display_id, o.scale_factor, o.image_path, o.image_blob_hash, o.thumbnail_path, \
    o.image_bytes, o.image_w, o.image_h, o.phash, o.change_kind, o.privacy_verdict, a.status";

impl Database {
    /// 写入一条观测，并在**同一事务内**追加对应事件、创建 pending 分析行。
    ///
    /// 幂等：命中 `idempotency` 唯一索引时不再写入，返回已存在那行的 id。
    pub fn insert_observation(&self, obs: &NewObservation) -> Result<InsertOutcome, AppError> {
        let payload = serde_json::to_string(&obs.event_payload()).map_err(|e| {
            AppError::new(
                mc_common::error::ErrorCode::StorageUnavailable,
                format!("事件 payload 序列化失败: {e}"),
            )
        })?;

        // phash 是 u64，而 SQLite 的 INTEGER 是有符号 64 位；按位往返而非数值转换
        let phash: Option<i64> = obs.phash.map(|v| v as i64);

        let mut conn = self.lock_write()?;
        let tx = conn.transaction().map_err(map_sqlite_error)?;

        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO observations (
                    id, ts_utc_ms, source_id, kind, app_name, app_bundle_id, window_title,
                    domain, display_id, scale_factor, image_path, image_blob_hash,
                    thumbnail_path, image_bytes, image_w, image_h, phash,
                    text_content, text_origin, change_kind, privacy_verdict,
                    idempotency, created_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20, ?21, ?22, ?23
                 )",
                rusqlite::params![
                    obs.id,
                    obs.ts.as_millis(),
                    obs.source_id,
                    obs.kind,
                    obs.app_name,
                    obs.app_bundle_id,
                    obs.window_title,
                    obs.domain,
                    obs.display_id,
                    obs.scale_factor,
                    obs.image.as_ref().map(|i| i.relative_path.clone()),
                    obs.image.as_ref().map(|i| i.content_hash.clone()),
                    obs.image.as_ref().and_then(|i| i.thumbnail_path.clone()),
                    obs.image.as_ref().map(|i| i.bytes as i64),
                    obs.image.as_ref().map(|i| i.width as i64),
                    obs.image.as_ref().map(|i| i.height as i64),
                    phash,
                    obs.text_content,
                    obs.text_origin,
                    obs.change_kind,
                    obs.privacy_verdict,
                    obs.idempotency,
                    obs.ts.as_millis(),
                ],
            )
            .map_err(map_sqlite_error)?;

        let id = if changed == 1 {
            obs.id.clone()
        } else {
            // 命中幂等键 → 复用已有行；找不到说明是**主键冲突**（不同幂等键、同 id），
            // 那是调用方的 bug，必须明确报错而不是静默当成「重复投递」。
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM observations WHERE idempotency = ?1",
                    [&obs.idempotency],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite_error)?;

            match existing {
                Some(existing_id) => existing_id,
                None => {
                    return Err(AppError::new(
                        mc_common::error::ErrorCode::StorageUnavailable,
                        format!("观测 id {} 与已有记录冲突（幂等键不同）", obs.id),
                    ))
                }
            }
        };

        if changed == 1 {
            tx.execute(
                "INSERT INTO events (at_utc_ms, kind, schema_version, payload, actor)
                 VALUES (?1, 'observation_captured', ?2, ?3, 'capture')",
                rusqlite::params![
                    obs.ts.as_millis(),
                    crate::events::EVENT_SCHEMA_VERSION as i64,
                    payload
                ],
            )
            .map_err(map_sqlite_error)?;

            tx.execute(
                "INSERT OR IGNORE INTO analyses (observation_id, status, tier)
                 VALUES (?1, 'pending', 'l0')",
                [&id],
            )
            .map_err(map_sqlite_error)?;
        }

        tx.commit().map_err(map_sqlite_error)?;

        Ok(InsertOutcome {
            id,
            inserted: changed == 1,
        })
    }

    /// 时间范围内的观测条数（任意时段总结的预览用）。
    pub fn count_observations_in_range(
        &self,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<u32, AppError> {
        self.with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM observations WHERE ts_utc_ms >= ?1 AND ts_utc_ms < ?2",
                rusqlite::params![from.as_millis(), to.as_millis()],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count as u32)
        })
    }

    /// 时间范围内被隐私规则拦下的观测条数。
    ///
    /// 预览必须把它报出来：用户有权知道「这段时间有多少内容没被总结」，
    /// 否则会以为系统漏了。
    pub fn count_blocked_observations_in_range(
        &self,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<u32, AppError> {
        self.with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM observations
                 WHERE ts_utc_ms >= ?1 AND ts_utc_ms < ?2 AND privacy_verdict = 'blocked'",
                rusqlite::params![from.as_millis(), to.as_millis()],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count as u32)
        })
    }

    /// 被隐私规则拦下的观测 id。
    ///
    /// 检索与问答用它做**失败即关闭**的判断：只要一条活动的证据里
    /// 含被拦截的观测，整条活动就不该出现在检索结果里。
    /// 逐条过滤是不够的 —— 标题里往往就带着应用名，拼起来照样能推断出内容。
    pub fn blocked_observation_ids(&self) -> Result<Vec<String>, AppError> {
        self.with_read(|conn| {
            let mut stmt =
                conn.prepare("SELECT id FROM observations WHERE privacy_verdict = 'blocked'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 按 id 批量取「有图」的观测的图片相对路径。
    ///
    /// 推断时要把截图喂给模型，而活动只记了观测 id ——
    /// 没有这一步，推断就只能靠窗口标题猜（那等于白花钱）。
    pub fn image_paths(&self, ids: &[String]) -> Result<Vec<(String, String)>, AppError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT image_path FROM observations
                 WHERE id = ?1 AND image_path IS NOT NULL",
            )?;

            let mut out = Vec::new();
            for id in ids {
                let path: Option<String> = stmt
                    .query_row([id], |row| row.get::<_, String>(0))
                    .optional()?;
                if let Some(path) = path {
                    out.push((id.clone(), path));
                }
            }
            Ok(out)
        })
    }

    pub fn query_observations(
        &self,
        query: &ObservationQuery,
    ) -> Result<Vec<ObservationRow>, AppError> {
        let mut sql = format!(
            "SELECT {SELECT_COLUMNS} FROM observations o
             LEFT JOIN analyses a ON a.observation_id = o.id
             WHERE 1 = 1"
        );
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(from) = query.from {
            sql.push_str(" AND o.ts_utc_ms >= ?");
            values.push(Box::new(from.as_millis()));
        }
        if let Some(to) = query.to {
            sql.push_str(" AND o.ts_utc_ms < ?");
            values.push(Box::new(to.as_millis()));
        }
        if let Some(app) = &query.app_name {
            sql.push_str(" AND o.app_name = ?");
            values.push(Box::new(app.clone()));
        }
        if query.exclude_blocked {
            sql.push_str(" AND o.privacy_verdict != 'blocked'");
        }

        sql.push_str(" ORDER BY o.ts_utc_ms DESC, o.id DESC LIMIT ? OFFSET ?");
        values.push(Box::new(query.limit.unwrap_or(DEFAULT_LIMIT) as i64));
        values.push(Box::new(query.offset.unwrap_or(0) as i64));

        self.with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), map_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn latest_observation(&self) -> Result<Option<ObservationRow>, AppError> {
        Ok(self
            .query_observations(&ObservationQuery {
                limit: Some(1),
                ..Default::default()
            })?
            .into_iter()
            .next())
    }

    pub fn observation_count(&self) -> Result<i64, AppError> {
        self.with_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM observations", [], |row| {
                row.get::<_, i64>(0)
            })
        })
    }

    /// 这里只暴露「待分析」队列长度，
    /// 用于观察「采集了但还没处理」的积压。
    pub fn pending_analysis_count(&self) -> Result<i64, AppError> {
        self.with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM analyses WHERE status = 'pending'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
    }
}

/// 按**列名**读取，而不是按下标。
///
/// 用下标时，任何一次 SELECT 列表调整都会静默地把字段读串（类型相同就查不出来）。
fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObservationRow> {
    // phash 是 u64，SQLite 的 INTEGER 是有符号 64 位 —— 按位往返
    let phash: Option<i64> = row.get("phash")?;

    Ok(ObservationRow {
        id: row.get("id")?,
        ts: Timestamp::from_millis(row.get("ts_utc_ms")?),
        source_id: row.get("source_id")?,
        kind: row.get("kind")?,
        app_name: row.get("app_name")?,
        app_bundle_id: row.get("app_bundle_id")?,
        window_title: row.get("window_title")?,
        display_id: row.get("display_id")?,
        scale_factor: row.get("scale_factor")?,
        image_path: row.get("image_path")?,
        image_blob_hash: row.get("image_blob_hash")?,
        thumbnail_path: row.get("thumbnail_path")?,
        image_bytes: row.get::<_, Option<i64>>("image_bytes")?.map(|v| v as u64),
        image_w: row.get::<_, Option<i64>>("image_w")?.map(|v| v as u32),
        image_h: row.get::<_, Option<i64>>("image_h")?.map(|v| v as u32),
        phash: phash.map(|v| v as u64),
        change_kind: row.get("change_kind")?,
        privacy_verdict: row.get("privacy_verdict")?,
        analysis_status: row.get("status")?,
    })
}
