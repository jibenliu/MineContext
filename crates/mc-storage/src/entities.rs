//! 实体关联落库。
//!
//! 复用 `0001_init.sql` 的 `entities` + `activity_entities`（迁移 `0006` 补 `display_name`
//! 与反查索引）。三条决定：
//!
//! 1. **按活动整体替换**（先删链接再重建）：追加式写入会把用户改标题前的旧实体
//!    留在表里，「相关记录」就是假的；
//! 2. **实体 id 确定性**（`{kind}:{canonical}`）：同步天然幂等，不需要先查再插；
//! 3. **孤儿实体同步清理**：没有链接的行只会让「被拦截的活动出现过」留在库里。
//!    展示名保留原文 —— 归并视 `apex-389` 与 `APEX-389` 为同一实体，但不改写它。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use rusqlite::{params, OptionalExtension};

use crate::db::Database;

/// 当前唯一使用的关联角色：实体在该活动标题里被提到。
pub const ROLE_MENTIONED: &str = "mentioned";

/// 一条「活动 ↔ 实体」关联。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEntity {
    pub activity_id: String,
    /// `issue` / `file_note`（见 `mc_memory::entity::EntityKind::as_str`）
    pub kind: String,
    /// 规范化名字（比较、归并用）
    pub key: String,
    /// 用户看到的样子（原文）
    pub display: String,
    /// 冗余出来的活动开始时间（来自 `activities` 表），线索按它排序
    pub start: Timestamp,
}

/// 确定性实体 id：同一 `(kind, canonical)` 永远得到同一行。
pub fn entity_id(kind: &str, canonical: &str) -> String {
    format!("{kind}:{canonical}")
}

impl Database {
    /// 批量替换变化的关联并清理不再允许的活动；整个批次在同一事务中提交。
    pub fn sync_activity_entity_links(
        &self,
        changes: &[(String, Vec<ActivityEntity>)],
        allowed: &[String],
    ) -> Result<usize, AppError> {
        let changed_ids =
            serde_json::json!(changes.iter().map(|(id, _)| id).collect::<Vec<_>>()).to_string();
        let allowed_ids = serde_json::json!(allowed).to_string();
        let rows: Vec<_> = changes
            .iter()
            .flat_map(|(id, entities)| {
                entities.iter().map(move |entity| {
                    serde_json::json!({
                        "activity_id": id,
                        "id": entity_id(&entity.kind, &entity.key),
                        "kind": entity.kind,
                        "key": entity.key,
                        "display": entity.display,
                        "start": entity.start.as_millis(),
                    })
                })
            })
            .collect();
        let rows = serde_json::json!(rows).to_string();
        self.with_write(|conn| {
            let tx = conn.transaction()?;
            let cleared: i64 = tx.query_row(
                "SELECT COUNT(DISTINCT activity_id) FROM activity_entities
                 WHERE activity_id NOT IN (SELECT value FROM json_each(?1))",
                [&allowed_ids], |row| row.get(0),
            )?;
            tx.execute(
                "DELETE FROM activity_entities WHERE activity_id IN (SELECT value FROM json_each(?1))
                 OR activity_id NOT IN (SELECT value FROM json_each(?2))",
                params![changed_ids, allowed_ids],
            )?;
            tx.execute(
                "INSERT INTO entities (id, kind, canonical, aliases, first_seen, last_seen, origin, display_name)
                 SELECT json_extract(item.value, '$.id'), json_extract(item.value, '$.kind'),
                        json_extract(item.value, '$.key'), NULL,
                        COALESCE(a.start_utc_ms, json_extract(item.value, '$.start')),
                        COALESCE(a.start_utc_ms, json_extract(item.value, '$.start')), 'rule',
                        json_extract(item.value, '$.display')
                 FROM json_each(?1) item LEFT JOIN activities a
                   ON a.id = json_extract(item.value, '$.activity_id') WHERE 1
                 ON CONFLICT(id) DO UPDATE SET
                   first_seen = MIN(entities.first_seen, excluded.first_seen),
                   last_seen = MAX(entities.last_seen, excluded.last_seen),
                   display_name = excluded.display_name",
                [&rows],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO activity_entities (activity_id, entity_id, role)
                 SELECT json_extract(value, '$.activity_id'), json_extract(value, '$.id'), ?2 FROM json_each(?1)",
                params![rows, ROLE_MENTIONED],
            )?;
            prune_orphans(&tx)?;
            tx.commit()?;
            Ok(cleared as usize)
        })
    }

    /// 用给定集合**整体替换**某个活动的实体关联，返回写入的关联数。
    ///
    /// 重复的 `(kind, key)` 会被去重（同一活动内同一个实体只留一行）。
    pub fn replace_activity_entities(
        &self,
        activity_id: &str,
        entities: &[ActivityEntity],
    ) -> Result<usize, AppError> {
        let mut seen = std::collections::HashSet::new();
        let unique: Vec<&ActivityEntity> = entities
            .iter()
            .filter(|entity| seen.insert((entity.kind.clone(), entity.key.clone())))
            .collect();

        self.with_write(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM activity_entities WHERE activity_id = ?1",
                [activity_id],
            )?;

            {
                let mut find_activity = tx.prepare("SELECT id FROM activities WHERE id = ?1")?;
                let known: bool = find_activity
                    .query_row([activity_id], |row| row.get::<_, String>(0))
                    .optional()?
                    .is_some();

                let mut upsert = tx.prepare(
                    "INSERT INTO entities
                       (id, kind, canonical, aliases, first_seen, last_seen, origin, display_name)
                     VALUES (?1, ?2, ?3, NULL, ?4, ?4, 'rule', ?5)
                     ON CONFLICT(id) DO UPDATE SET
                         first_seen   = MIN(entities.first_seen, excluded.first_seen),
                         last_seen    = MAX(entities.last_seen, excluded.last_seen),
                         display_name = excluded.display_name",
                )?;
                let mut link = tx.prepare(
                    "INSERT OR IGNORE INTO activity_entities (activity_id, entity_id, role)
                     VALUES (?1, ?2, ?3)",
                )?;

                for entity in &unique {
                    let id = entity_id(&entity.kind, &entity.key);
                    // 开始时间优先用活动表里的真值；活动还没落库时退回实体自带的时间
                    let start = if known {
                        tx.query_row(
                            "SELECT start_utc_ms FROM activities WHERE id = ?1",
                            [activity_id],
                            |row| row.get::<_, i64>(0),
                        )?
                    } else {
                        entity.start.as_millis()
                    };

                    upsert.execute(params![id, entity.kind, entity.key, start, entity.display,])?;
                    link.execute(params![activity_id, id, ROLE_MENTIONED])?;
                }
            }

            prune_orphans(&tx)?;
            tx.commit()?;
            Ok(unique.len())
        })
    }

    /// 清空某个活动的实体关联（活动被删除、或标题不再含任何实体时）。
    ///
    /// 同时清掉不再被任何活动引用的实体行。
    pub fn clear_activity_entities(&self, activity_id: &str) -> Result<usize, AppError> {
        self.with_write(|conn| {
            let tx = conn.transaction()?;
            let removed = tx.execute(
                "DELETE FROM activity_entities WHERE activity_id = ?1",
                [activity_id],
            )?;
            prune_orphans(&tx)?;
            tx.commit()?;
            Ok(removed)
        })
    }

    /// 某个活动的全部实体（按 kind、key 稳定排序）。
    pub fn entities_for_activity(
        &self,
        activity_id: &str,
    ) -> Result<Vec<ActivityEntity>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ae.activity_id, e.kind, e.canonical,
                        COALESCE(e.display_name, e.canonical), a.start_utc_ms
                   FROM activity_entities ae
                   JOIN entities   e ON e.id = ae.entity_id
                   JOIN activities a ON a.id = ae.activity_id
                  WHERE ae.activity_id = ?1
                  ORDER BY e.kind ASC, e.canonical ASC",
            )?;
            let rows = stmt.query_map([activity_id], row_to_entity)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 反查：某个实体出现在哪些活动里（时间升序）。
    ///
    /// 线索（Thread）与「APEX-389 到哪了」这类问题都走这条路径。
    pub fn activities_for_entity(
        &self,
        kind: &str,
        canonical: &str,
    ) -> Result<Vec<ActivityEntity>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ae.activity_id, e.kind, e.canonical,
                        COALESCE(e.display_name, e.canonical), a.start_utc_ms
                   FROM entities e
                   JOIN activity_entities ae ON ae.entity_id = e.id
                   JOIN activities a ON a.id = ae.activity_id
                  WHERE e.kind = ?1 AND e.canonical = ?2
                  ORDER BY a.start_utc_ms ASC, ae.activity_id ASC",
            )?;
            let rows = stmt.query_map(params![kind, canonical], row_to_entity)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 全部关联（供诊断与同步比对），按活动 + 实体稳定排序。
    pub fn all_activity_entities(&self) -> Result<Vec<ActivityEntity>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ae.activity_id, e.kind, e.canonical,
                        COALESCE(e.display_name, e.canonical), a.start_utc_ms
                   FROM activity_entities ae
                   JOIN entities   e ON e.id = ae.entity_id
                   JOIN activities a ON a.id = ae.activity_id
                  ORDER BY ae.activity_id ASC, e.kind ASC, e.canonical ASC",
            )?;
            let rows = stmt.query_map([], row_to_entity)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn activity_entity_count(&self) -> Result<usize, AppError> {
        let count: i64 = self.with_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM activity_entities", [], |row| {
                row.get(0)
            })
        })?;
        Ok(count as usize)
    }

    pub fn activity_entities_for_ids(
        &self,
        activity_ids: &[String],
    ) -> Result<Vec<ActivityEntity>, AppError> {
        if activity_ids.is_empty() {
            return Ok(Vec::new());
        }
        let selected = serde_json::json!(activity_ids).to_string();
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ae.activity_id, e.kind, e.canonical,
                        COALESCE(e.display_name, e.canonical), a.start_utc_ms
                   FROM activity_entities ae
                   JOIN entities e ON e.id = ae.entity_id
                   JOIN activities a ON a.id = ae.activity_id
                  WHERE ae.activity_id IN (SELECT value FROM json_each(?1))
                  ORDER BY ae.activity_id ASC, e.kind ASC, e.canonical ASC",
            )?;
            let rows = stmt.query_map([selected], row_to_entity)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 库里现存的实体数量（去重后的实体，不是关联数）。
    pub fn entity_count(&self) -> Result<usize, AppError> {
        let count: i64 = self.with_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))
        })?;
        Ok(count as usize)
    }
}

/// 清掉不再被任何活动引用的实体行。
///
/// 不清理的后果不只是「表变大」：被拦截的活动删除后，
/// 它对应的实体行会留在库里 —— 那是关于被拦截内容的一份残留记录。
fn prune_orphans(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<usize> {
    tx.execute(
        "DELETE FROM entities WHERE id NOT IN (SELECT entity_id FROM activity_entities)",
        [],
    )
}

fn row_to_entity(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActivityEntity> {
    Ok(ActivityEntity {
        activity_id: row.get(0)?,
        kind: row.get(1)?,
        key: row.get(2)?,
        display: row.get(3)?,
        start: Timestamp::from_millis(row.get(4)?),
    })
}
