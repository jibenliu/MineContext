//! 向量持久化。
//!
//! 立场：**向量是派生数据，不是真源**（真源是 `events` 与 `observations`），
//! 因此整张 `vectors` 表可以随时清空重建，主链路没有感知。
//!
//! - 维度**不是常量**：库里记着当前向量空间（模型 + 维度），换模型时立刻报
//!   [`ErrorCode::StorageEmbeddingDimensionMismatch`] 并提示重建，而不是静默返回空；
//! - 重建**可续**：水位线按 `kind` 落库，重复投递同一文档是覆盖（主键
//!   `(kind, doc_id, model)`）；存储是 f32 小端 BLOB（JSON 掉精度且体积约 3 倍），
//!   当前用暴力余弦检索，规模结论见 `decisions/vector-scale.md`。

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use rusqlite::{params, OptionalExtension};

use crate::db::Database;

/// 一条待写入/已读出的向量。
#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    /// 文档类别：`activity` / `summary` / `document`（与 `DocumentKind::as_str()` 一致）
    pub kind: String,
    pub doc_id: String,
    /// 产生这条向量的模型名。换模型会写出新行，旧模型的向量不被静默复用。
    pub model: String,
    pub values: Vec<f32>,
}

impl VectorRecord {
    pub fn dimensions(&self) -> usize {
        self.values.len()
    }
}

/// 当前向量空间：由**第一条真实向量**（或显式声明）决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingSpaceRecord {
    pub model: String,
    pub dimensions: usize,
}

const SPACE_KEY: &str = "embedding_space";

/// 归一化：`kind` 与 `model` 统一小写，避免 `Activity` / `activity` 各写一份。
fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn encode(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn dimension_mismatch(
    stored: &EmbeddingSpaceRecord,
    incoming_model: &str,
    incoming: usize,
) -> AppError {
    AppError::new(
        ErrorCode::StorageEmbeddingDimensionMismatch,
        format!(
            "已存向量空间是 {} 维（模型 {}），新向量是 {} 维（模型 {}）。\
             换 embedding 模型后必须重建向量索引。",
            stored.dimensions, stored.model, incoming, incoming_model
        ),
    )
    .with_context("stored_dimensions", stored.dimensions.to_string())
    .with_context("stored_model", stored.model.clone())
    .with_context("incoming_dimensions", incoming.to_string())
    .with_context("incoming_model", incoming_model.to_string())
}

/// 读取当前向量空间。空库（或被 reset）时为 `None`。
pub fn embedding_space(db: &Database) -> Result<Option<EmbeddingSpaceRecord>, AppError> {
    let raw: Option<String> = db.with_read(|conn| {
        conn.query_row(
            "SELECT value FROM vector_state WHERE key = ?1",
            [SPACE_KEY],
            |row| row.get(0),
        )
        .optional()
    })?;

    let Some(raw) = raw else {
        return Ok(None);
    };

    let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        AppError::new(
            ErrorCode::StorageCorrupt,
            format!("vector_state.embedding_space 不是合法 JSON：{e}"),
        )
    })?;

    let model = parsed["model"].as_str().unwrap_or_default().to_string();
    let dimensions = parsed["dimensions"].as_u64().unwrap_or(0) as usize;
    if model.is_empty() || dimensions == 0 {
        return Err(AppError::new(
            ErrorCode::StorageCorrupt,
            format!("vector_state.embedding_space 缺少 model/dimensions：{raw}"),
        ));
    }
    Ok(Some(EmbeddingSpaceRecord { model, dimensions }))
}

/// 显式声明向量空间（用于「先探测 provider 维度，再决定是否重建」的启动路径）。
///
/// 与已有声明不一致时返回可执行的错误 —— 用户必须显式 `reset_vectors()`，
/// 因为静默接受新维度会让旧向量全部变成噪声。
pub fn declare_embedding_space(
    db: &Database,
    model: &str,
    dimensions: usize,
    now: Timestamp,
) -> Result<EmbeddingSpaceRecord, AppError> {
    if dimensions == 0 {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            "embedding 维度不能为 0",
        ));
    }

    let model = normalize(model);
    let existing = embedding_space(db)?;
    if let Some(existing) = existing.as_ref() {
        let same = existing.dimensions == dimensions && existing.model == model;
        if !same {
            return Err(dimension_mismatch(existing, &model, dimensions));
        }
        return Ok(existing.clone());
    }

    write_space(db, &model, dimensions, now)?;
    Ok(EmbeddingSpaceRecord { model, dimensions })
}

fn write_space(
    db: &Database,
    model: &str,
    dimensions: usize,
    now: Timestamp,
) -> Result<(), AppError> {
    let value = serde_json::json!({ "model": model, "dimensions": dimensions }).to_string();
    let at = now.to_legacy_datetime();
    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO vector_state (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![SPACE_KEY, value, at],
        )?;
        Ok(())
    })
}

/// 向量表的版本戳：`COUNT(*)` + `MAX(updated_at)`，给语义索引缓存当失效键。
///
/// 两个都要：插入/删除改 count，**重算同一条文档的向量只改 updated_at**。
/// 只用 count 会把陈旧向量当成新向量去检索（静默给错引用）。
pub fn vectors_version(db: &Database) -> Result<String, AppError> {
    db.with_read(|conn| {
        let (count, latest): (i64, Option<String>) =
            conn.query_row("SELECT COUNT(*), MAX(updated_at) FROM vectors", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        Ok(format!("{count}:{}", latest.unwrap_or_default()))
    })
}

/// 写入（或覆盖）向量。维度与已声明空间不一致时**整批拒绝**，不写入半截数据。
///
/// 返回实际写入的行数。
pub fn upsert_vectors(
    db: &Database,
    records: &[VectorRecord],
    now: Timestamp,
) -> Result<usize, AppError> {
    if records.is_empty() {
        return Ok(0);
    }

    // 先做全部校验，再统一写入：中途失败不能留下「一半是新维度、一半是旧维度」。
    let mut space = embedding_space(db)?;
    for record in records {
        if record.values.is_empty() {
            return Err(AppError::new(
                ErrorCode::DomainInvalidRange,
                format!(
                    "文档 {} 的向量为空，空向量无法参与相似度计算",
                    record.doc_id
                ),
            )
            .with_context("doc_id", record.doc_id.clone())
            .with_context("kind", record.kind.clone()));
        }
        let model = normalize(&record.model);
        match space.as_ref() {
            Some(existing) => {
                if existing.dimensions != record.dimensions() || existing.model != model {
                    return Err(dimension_mismatch(existing, &model, record.dimensions()));
                }
            }
            None => {
                space = Some(EmbeddingSpaceRecord {
                    model: model.clone(),
                    dimensions: record.dimensions(),
                });
            }
        }
    }

    // 空库（或刚被 reset）：从第一条向量声明向量空间
    if embedding_space(db)?.is_none() {
        let space = space.as_ref().expect("上面已经填好了向量空间");
        write_space(db, &space.model, space.dimensions, now)?;
    }

    let at = now.to_legacy_datetime();
    db.with_write(|conn| {
        let tx = conn.transaction()?;
        let mut written = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO vectors (kind, doc_id, model, dimensions, vector, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(kind, doc_id, model) DO UPDATE SET
                     dimensions = excluded.dimensions,
                     vector     = excluded.vector,
                     updated_at = excluded.updated_at",
            )?;
            for record in records {
                stmt.execute(params![
                    normalize(&record.kind),
                    record.doc_id,
                    normalize(&record.model),
                    record.dimensions() as i64,
                    encode(&record.values),
                    at,
                ])?;
                written += 1;
            }
        }
        tx.commit()?;
        Ok(written)
    })
}

/// 读取某个 `kind` 的全部向量，按 `doc_id` 稳定排序（检索结果必须可复现）。
pub fn load_vectors(db: &Database, kind: &str) -> Result<Vec<VectorRecord>, AppError> {
    let kind = normalize(kind);
    db.with_read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT kind, doc_id, model, vector FROM vectors WHERE kind = ?1 ORDER BY doc_id ASC, model ASC",
        )?;
        let rows = stmt.query_map([kind], |row| {
            let kind: String = row.get(0)?;
            let doc_id: String = row.get(1)?;
            let model: String = row.get(2)?;
            let bytes: Vec<u8> = row.get(3)?;
            Ok(VectorRecord {
                kind,
                doc_id,
                model,
                values: decode(&bytes),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

pub fn count_vectors(db: &Database, kind: &str) -> Result<usize, AppError> {
    let kind = normalize(kind);
    let count: i64 = db.with_read(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM vectors WHERE kind = ?1",
            [kind],
            |row| row.get(0),
        )
    })?;
    Ok(count as usize)
}

/// 删除单个文档的向量（活动被合并/删除时调用）。返回删除行数。
pub fn delete_vectors(db: &Database, kind: &str, doc_id: &str) -> Result<usize, AppError> {
    let kind = normalize(kind);
    db.with_write(|conn| {
        conn.execute(
            "DELETE FROM vectors WHERE kind = ?1 AND doc_id = ?2",
            params![kind, doc_id],
        )
    })
}

/// 清空某个 `kind` 的向量（该类别需要整体重建时调用）。
pub fn clear_vectors(db: &Database, kind: &str) -> Result<usize, AppError> {
    let kind = normalize(kind);
    db.with_write(|conn| conn.execute("DELETE FROM vectors WHERE kind = ?1", [kind]))
}

/// 全量重置：清空所有向量、向量空间声明与重建水位线。
///
/// 等价于「用户删掉了向量库」。**不碰真源**（events/observations/activities），
/// 因此调用方可以直接重新嵌入恢复。
pub fn reset_vectors(db: &Database) -> Result<usize, AppError> {
    db.with_write(|conn| {
        let tx = conn.transaction()?;
        let removed = tx.execute("DELETE FROM vectors", [])?;
        tx.execute("DELETE FROM vector_state", [])?;
        tx.commit()?;
        Ok(removed)
    })
}

fn rebuild_key(kind: &str) -> String {
    format!("rebuild_watermark:{}", normalize(kind))
}

/// 重建水位线：已索引到的**事件序号**。`None` 表示还没开始（或已被重置）。
pub fn rebuild_watermark(db: &Database, kind: &str) -> Result<Option<i64>, AppError> {
    let key = rebuild_key(kind);
    let raw: Option<String> = db.with_read(|conn| {
        conn.query_row(
            "SELECT value FROM vector_state WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
    })?;
    Ok(raw.and_then(|value| value.parse::<i64>().ok()))
}

/// 推进重建水位线（只允许前进，倒退说明调用方算错了 —— 会重复索引、白烧 token）。
pub fn set_rebuild_watermark(
    db: &Database,
    kind: &str,
    seq: i64,
    now: Timestamp,
) -> Result<(), AppError> {
    let previous = rebuild_watermark(db, kind)?;
    if previous.is_some_and(|previous| seq < previous) {
        let previous = previous.expect("上面已确认存在");
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("重建水位线只能前进：当前 {previous}，收到 {seq}"),
        ));
    }

    let key = rebuild_key(kind);
    let at = now.to_legacy_datetime();
    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO vector_state (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, seq.to_string(), at],
        )?;
        Ok(())
    })
}
