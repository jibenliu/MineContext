//! `activities` 投影器：把 `Projection` 落进派生表。
//!
//! **覆盖式写入**：先清空 `activities` / `activity_observations`，再整份写入。
//! 之所以不用增量 upsert：投影结果是纯函数的完整输出，增量更新只会引入
//! 「上一次留下的脏行」这类只有线上才暴露的问题。
//!
//! 位点（`projection_checkpoints`）与派生表在**同一事务**内推进，
//! 因此不存在「位点说投影过了、表里却没有数据」的永久空洞。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_domain::projector::{DomainEvent, Projection, ProjectionOptions};
use mc_domain::rules::RuleSet;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::events::EventEnvelope;

/// 投影器名字：位点表的主键，也是「哪个投影器落后了」的诊断入口。
pub const PROJECTOR: &str = "activities";

/// 落库之后的活动（列形状与 `activities` 表一致）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredActivity {
    pub id: String,
    /// 对外暴露的整数 id。由投影顺序决定，重放后保持不变。
    pub legacy_id: i64,
    pub start: Timestamp,
    pub end: Timestamp,
    pub title: String,
    /// 用户改名前的算法标题；为 `None` 表示用户没改过
    pub original_title: Option<String>,
    pub category: Option<String>,
    pub origin: Provenance,
    pub confidence: f32,
    /// 证据：观测 id 列表
    pub evidence: Vec<String>,
    /// 与 `evidence` 相同，但保留列顺序语义（关联表）
    pub observations: Vec<String>,
    pub derived_from_seq: i64,
}

impl StoredActivity {
    /// 用户是否改过（标题与算法原标题不一致）。
    pub fn is_user_modified(&self) -> bool {
        self.original_title
            .as_deref()
            .is_some_and(|original| original != self.title)
    }
}

/// 兼容 `activity` 表里「这一行是本投影器写的」的标记。
///
/// 用它把**我们写的行**和「从旧库迁移过来的行」区分开：
/// 前者每次投影清掉重写，后者必须原样留着（旧库 id 是主键，撞了会直接丢数据）。
const PROJECTOR_MARKER: &str = "%\"projector\":\"activities\"%";

/// 覆盖式写入投影结果，并与位点同事务提交。
pub fn store(
    db: &Database,
    projection: &Projection,
    last_seq: i64,
    at: Timestamp,
) -> Result<(), AppError> {
    db.with_write(|conn| {
        let tx = conn.transaction()?;

        tx.execute("DELETE FROM activity_observations", [])?;
        tx.execute("DELETE FROM activities", [])?;

        // 兼容表里别人的行（从旧库迁移过来的历史）要留着，而且我们的 id 必须排在
        // 它们后面，否则直接主键冲突丢数据。这样重放两次得到的 id 也一样。
        let legacy_base: i64 = tx.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM activity
             WHERE COALESCE(metadata, '') NOT LIKE ?1",
            rusqlite::params![PROJECTOR_MARKER],
            |row| row.get(0),
        )?;
        tx.execute(
            "DELETE FROM activity WHERE COALESCE(metadata, '') LIKE ?1",
            rusqlite::params![PROJECTOR_MARKER],
        )?;

        for (index, view) in projection.activities.iter().enumerate() {
            // legacy_id 由投影顺序决定 —— 用 AUTOINCREMENT 会随重放次数漂移，
            // 而对外暴露的整数 id 必须在重放后保持不变。
            let legacy_id = legacy_base + index as i64 + 1;
            let (origin, origin_ref) = origin_columns(&view.origin);
            let evidence =
                serde_json::to_string(&view.observation_ids()).unwrap_or_else(|_| "[]".to_string());
            let original_title = if view.original_title == view.title {
                None
            } else {
                Some(view.original_title.clone())
            };

            tx.execute(
                "INSERT INTO activities
                   (id, legacy_id, start_utc_ms, end_utc_ms, title, original_title, category,
                    origin, origin_ref, confidence, evidence, entity_ids, derived_from_seq,
                    superseded_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, NULL)",
                rusqlite::params![
                    view.id,
                    legacy_id,
                    view.start.as_millis(),
                    view.end.as_millis(),
                    view.title,
                    original_title,
                    view.category,
                    origin,
                    origin_ref,
                    view.confidence,
                    evidence,
                    last_seq,
                ],
            )?;

            for observation in &view.observations {
                tx.execute(
                    "INSERT INTO activity_observations (activity_id, observation_id)
                     VALUES (?1, ?2)",
                    rusqlite::params![view.id, observation.id],
                )?;
            }

            store_legacy_row(&tx, legacy_id, view)?;
        }

        tx.execute(
            "INSERT INTO projection_checkpoints (projector, last_seq, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(projector) DO UPDATE
               SET last_seq = excluded.last_seq, updated_at = excluded.updated_at",
            rusqlite::params![PROJECTOR, last_seq, at.as_millis()],
        )?;

        tx.commit()?;
        Ok(())
    })
}

/// 清空派生表（重放前用）。位点也一并清掉，避免「位点说已投影、表是空的」。
pub fn clear(db: &Database) -> Result<(), AppError> {
    db.with_write(|conn| {
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM activity_observations", [])?;
        tx.execute("DELETE FROM activities", [])?;
        // 只删我们写的行：迁移过来的旧数据不归投影器管
        tx.execute(
            "DELETE FROM activity WHERE COALESCE(metadata, '') LIKE ?1",
            rusqlite::params![PROJECTOR_MARKER],
        )?;
        tx.execute(
            "DELETE FROM projection_checkpoints WHERE projector = ?1",
            rusqlite::params![PROJECTOR],
        )?;
        tx.commit()?;
        Ok(())
    })
}

pub fn checkpoint(db: &Database) -> Result<Option<i64>, AppError> {
    db.with_read(|conn| {
        conn.query_row(
            "SELECT last_seq FROM projection_checkpoints WHERE projector = ?1",
            rusqlite::params![PROJECTOR],
            |row| row.get::<_, i64>("last_seq"),
        )
        .optional()
    })
}

pub fn link_count(db: &Database) -> Result<i64, AppError> {
    db.with_read(|conn| {
        conn.query_row("SELECT COUNT(*) FROM activity_observations", [], |row| {
            row.get::<_, i64>(0)
        })
    })
}

/// 按时间顺序读回全部活动（含观测关联）。
pub fn read_all(db: &Database) -> Result<Vec<StoredActivity>, AppError> {
    db.with_read(|conn| {
        let mut links: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT activity_id, observation_id FROM activity_observations
                 ORDER BY activity_id, observation_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>("activity_id")?,
                    row.get::<_, String>("observation_id")?,
                ))
            })?;
            for row in rows {
                let (activity_id, observation_id) = row?;
                links.entry(activity_id).or_default().push(observation_id);
            }
        }

        let mut stmt = conn.prepare(
            "SELECT id, legacy_id, start_utc_ms, end_utc_ms, title, original_title,
                    category, origin, origin_ref, confidence, evidence, derived_from_seq
             FROM activities
             ORDER BY start_utc_ms, id",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get("id")?;
            let start = Timestamp::from_millis(row.get::<_, i64>("start_utc_ms")?);
            let end_raw: Option<i64> = row.get("end_utc_ms")?;
            let evidence: String = row.get("evidence")?;
            let observations = links.get(&id).cloned().unwrap_or_default();

            Ok(StoredActivity {
                id,
                legacy_id: row.get("legacy_id")?,
                start,
                end: Timestamp::from_millis(end_raw.unwrap_or_else(|| start.as_millis())),
                title: row.get("title")?,
                original_title: row.get("original_title")?,
                category: row.get("category")?,
                origin: decode_origin(
                    &row.get::<_, String>("origin")?,
                    row.get::<_, Option<String>>("origin_ref")?,
                ),
                confidence: row.get::<_, Option<f64>>("confidence")?.unwrap_or(1.0) as f32,
                evidence: serde_json::from_str(&evidence).unwrap_or_default(),
                observations,
                derived_from_seq: row.get("derived_from_seq")?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
    })
}

/// 按 id 批量读活动（总结要把阶段里的活动还原成输入）。
///
/// 顺序由调用方给（阶段的活动顺序有含义），因此这里保持输入顺序。
pub fn read_by_ids(db: &Database, ids: &[String]) -> Result<Vec<StoredActivity>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let all: std::collections::HashMap<String, StoredActivity> = read_all(db)?
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect();

    Ok(ids.iter().filter_map(|id| all.get(id).cloned()).collect())
}

/// 读全部事件并解码成领域事件（阶段重放需要锁屏/睡眠事件）。
pub fn domain_events(db: &Database) -> Result<Vec<DomainEvent>, AppError> {
    Ok(read_all_events(db)?
        .iter()
        .map(|envelope| DomainEvent::from_stored(&envelope.kind, envelope.at, &envelope.payload))
        .collect())
}

/// 从事件日志全量重放并落库。
///
/// 这是 `mc-cli replay` 与「算法升级后重算」共用的入口：
/// 读全部事件 → 纯投影 → 覆盖写入 + 推进位点。
pub fn replay(
    db: &Database,
    rules: &RuleSet,
    options: ProjectionOptions,
    at: Timestamp,
) -> Result<Projection, AppError> {
    let envelopes = read_all_events(db)?;
    let last_seq = envelopes.last().map(|envelope| envelope.seq).unwrap_or(0);
    let events: Vec<DomainEvent> = envelopes
        .iter()
        .map(|envelope| DomainEvent::from_stored(&envelope.kind, envelope.at, &envelope.payload))
        .collect();

    let projection = mc_domain::projector::project(&events, rules, options);
    store(db, &projection, last_seq, at)?;
    Ok(projection)
}

/// 有新事件才重算。返回 `None` 表示日志没变、什么都没做。
///
/// 为什么要这一步：投影是**全量**重算（简单、不会留脏行），
/// 而定时任务每 30 秒会调一次 —— 没有这个判断就是纯粹的浪费。
pub fn project_if_dirty(
    db: &Database,
    rules: &RuleSet,
    options: ProjectionOptions,
    at: Timestamp,
) -> Result<Option<Projection>, AppError> {
    let last_seq = db.last_seq()?;
    let projected = checkpoint(db)?.unwrap_or(0);
    if projected >= last_seq {
        return Ok(None);
    }
    replay(db, rules, options, at).map(Some)
}

fn read_all_events(db: &Database) -> Result<Vec<EventEnvelope>, AppError> {
    // 分页读取，避免一次把百万级事件全塞进内存
    const PAGE: usize = 5_000;
    let mut out = Vec::new();
    let mut cursor = 0i64;
    loop {
        let page = db.read_events(cursor, PAGE)?;
        let Some(last) = page.last() else {
            break;
        };
        cursor = last.seq;
        let done = page.len() < PAGE;
        out.extend(page);
        if done {
            break;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- 兼容 activity 表

/// 把活动写成兼容 `activity` 表的一行。
///
/// 三个坑都在这里：
/// 1. `resources` / `metadata` 必须是 **JSON 字符串**（前端直接 `JSON.parse(item.resources)`）；
/// 2. `start_time` / `end_time` 必须是 **UTC 的 `YYYY-MM-DD HH:mm:ss`**
///    （前端用 dayjs 解析，换格式就是 Invalid Date）；
/// 3. `id` 由调用方显式给出，不用 AUTOINCREMENT（重放要稳定）。
fn store_legacy_row(
    tx: &rusqlite::Transaction<'_>,
    legacy_id: i64,
    view: &mc_domain::activity::ActivityView,
) -> rusqlite::Result<()> {
    let resources =
        serde_json::to_string(&image_resources(tx, view)?).unwrap_or_else(|_| "[]".to_string());

    let metadata = serde_json::to_string(&serde_json::json!({
        "projector": PROJECTOR,
        "activity_id": view.id,
        "category": view.category,
        "origin": origin_kind(&view.origin),
        "origin_ref": origin_columns(&view.origin).1,
        "confidence": view.confidence,
        "observations": view.observation_ids(),
        "is_user_modified": view.original_title != view.title,
        "original_title": view.original_title,
    }))
    .unwrap_or_else(|_| "{}".to_string());

    tx.execute(
        "INSERT INTO activity (id, title, content, resources, metadata, start_time, end_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            legacy_id,
            view.title,
            legacy_content(view),
            resources,
            metadata,
            format_legacy_datetime(view.start),
            format_legacy_datetime(view.end),
        ],
    )?;
    Ok(())
}

/// 兼容表的 `resources`：读取方只认 `{type, id, path}`，
/// 并且只把 `type == "image"` 的项渲染成缩略图。
fn image_resources(
    tx: &rusqlite::Transaction<'_>,
    view: &mc_domain::activity::ActivityView,
) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt =
        tx.prepare("SELECT image_path FROM observations WHERE id = ?1 AND image_path IS NOT NULL")?;

    let mut out = Vec::new();
    for observation in &view.observations {
        let path: Option<String> = stmt
            .query_row(rusqlite::params![observation.id], |row| row.get(0))
            .optional()?;
        if let Some(path) = path {
            out.push(serde_json::json!({
                "type": "image",
                "id": observation.id,
                "path": path,
            }));
        }
    }
    Ok(out)
}

/// 兼容表的 `content`：给一段确定性的摘要（由活动本身推导）。
/// 留空会让 hover 弹层整个失效，而模型总结不属于这一层。
fn legacy_content(view: &mc_domain::activity::ActivityView) -> String {
    let mut parts = Vec::new();
    if let Some(category) = &view.category {
        if !category.trim().is_empty() {
            parts.push(category.clone());
        }
    }
    parts.push(format!("{} 条观测", view.observations.len()));
    parts.join(" · ")
}

/// UTC 的 `YYYY-MM-DD HH:mm:ss` —— 兼容面里日期时间一律是这个格式。
fn format_legacy_datetime(at: Timestamp) -> String {
    chrono::DateTime::from_timestamp_millis(at.as_millis())
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn origin_kind(origin: &Provenance) -> &'static str {
    origin_columns(origin).0
}

// ---------------------------------------------------------------- 兼容层读取

/// 兼容 `activity` 表的一行。
///
/// `resources` / `metadata` **原样返回字符串**：读取方自己 `JSON.parse`，
/// 提前解析成对象反而会让它拿到 `[object Object]`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegacyActivity {
    pub id: i64,
    pub title: String,
    pub content: Option<String>,
    pub resources: Option<String>,
    pub metadata: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

const LEGACY_COLUMNS: &str = "id, title, content, resources, metadata, start_time, end_time";

/// `getAllActivities` / `getNewActivities` 的共用读取。
///
/// 范围是 `start_time > from AND start_time < to`（**严格不等**），按 `start_time`
/// 升序。时间参数用同样的字符串格式比较，因此写入后立刻查询即可命中。
pub fn read_legacy(
    db: &Database,
    range: Option<(Timestamp, Timestamp)>,
) -> Result<Vec<LegacyActivity>, AppError> {
    let sql = match range {
        Some(_) => format!(
            "SELECT {LEGACY_COLUMNS} FROM activity
             WHERE start_time > ?1 AND start_time < ?2 ORDER BY start_time ASC"
        ),
        None => format!("SELECT {LEGACY_COLUMNS} FROM activity ORDER BY start_time ASC"),
    };

    db.with_read(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(LegacyActivity {
                id: row.get("id")?,
                title: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
                content: row.get("content")?,
                resources: row.get("resources")?,
                metadata: row.get("metadata")?,
                start_time: row.get("start_time")?,
                end_time: row.get("end_time")?,
            })
        };

        let rows = match range {
            Some((from, to)) => {
                let params =
                    rusqlite::params![format_legacy_datetime(from), format_legacy_datetime(to)];
                stmt.query_map(params, map_row)?
                    .collect::<Result<Vec<_>, _>>()?
            }
            None => stmt
                .query_map([], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    })
}

/// `getLatestActivity`：按 id 倒序取首行。
pub fn latest_legacy(db: &Database) -> Result<Option<LegacyActivity>, AppError> {
    db.with_read(|conn| {
        conn.query_row(
            &format!("SELECT {LEGACY_COLUMNS} FROM activity ORDER BY id DESC LIMIT 1"),
            [],
            |row| {
                Ok(LegacyActivity {
                    id: row.get("id")?,
                    title: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
                    content: row.get("content")?,
                    resources: row.get("resources")?,
                    metadata: row.get("metadata")?,
                    start_time: row.get("start_time")?,
                    end_time: row.get("end_time")?,
                })
            },
        )
        .optional()
    })
}

fn origin_columns(origin: &Provenance) -> (&'static str, Option<String>) {
    match origin {
        Provenance::Observed => ("observed", None),
        Provenance::Rule { rule_id } => ("rule", Some(rule_id.clone())),
        Provenance::Inferred { model } => ("inferred", Some(model.clone())),
    }
}

fn decode_origin(origin: &str, origin_ref: Option<String>) -> Provenance {
    match origin {
        "rule" => Provenance::Rule {
            rule_id: origin_ref.unwrap_or_default(),
        },
        "inferred" => Provenance::Inferred {
            model: origin_ref.unwrap_or_default(),
        },
        _ => Provenance::Observed,
    }
}
