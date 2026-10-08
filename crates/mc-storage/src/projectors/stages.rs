//! `stages` 投影器：把阶段落到派生表。
//!
//! 阶段与总结都是**派生数据**：随时可以丢掉、用事件与活动重算。
//! 因此这里不做增量补丁，只做「写入 / 覆盖 / 查询」三件事。
//!
//! 巡检（第三道防线）依赖这里的一个查询：
//! **已关闭、但没有任何总结、且超过 deadline 的阶段** ——
//! 它就是「有阶段必有总结」这条不变量的最后一道兜底。

use mc_common::error::AppError;
use mc_common::time::Timestamp;

use crate::db::Database;

/// 落库之后的阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRow {
    pub id: String,
    pub start: Timestamp,
    pub end: Option<Timestamp>,
    /// `open` | `closed`
    pub state: String,
    /// `switched` | `idle` | `locked` | ... | `interrupted`
    pub end_reason: Option<String>,
    pub day: String,
    pub activities: Vec<String>,
}

impl StageRow {
    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }
}

impl Database {
    /// 写入 / 覆盖一个阶段及其活动关联。
    ///
    /// `activities` 的顺序有含义（时间顺序），因此关联表带 `position`，
    /// 读回来的时候按它排序 —— 否则总结里的活动顺序会随数据库返回顺序变化。
    pub fn upsert_stage(&self, stage: &StageRow, derived_from_seq: i64) -> Result<(), AppError> {
        let activities = stage.activities.clone();
        let stage = stage.clone();

        self.with_write(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO stages
                   (id, activity_id, start_utc_ms, end_utc_ms, state, end_reason, day,
                    derived_from_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                   activity_id = excluded.activity_id,
                   start_utc_ms = excluded.start_utc_ms,
                   end_utc_ms = excluded.end_utc_ms,
                   state = excluded.state,
                   end_reason = excluded.end_reason,
                   day = excluded.day,
                   derived_from_seq = excluded.derived_from_seq",
                rusqlite::params![
                    stage.id,
                    activities.first(),
                    stage.start.as_millis(),
                    stage.end.map(|end| end.as_millis()),
                    stage.state,
                    stage.end_reason,
                    stage.day,
                    derived_from_seq,
                ],
            )?;

            tx.execute(
                "DELETE FROM stage_activities WHERE stage_id = ?1",
                rusqlite::params![stage.id],
            )?;
            for (position, activity_id) in activities.iter().enumerate() {
                tx.execute(
                    "INSERT OR IGNORE INTO stage_activities (stage_id, activity_id, position)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![stage.id, activity_id, position as i64],
                )?;
            }

            tx.commit()?;
            Ok(())
        })
    }

    pub fn upsert_stages(
        &self,
        stages: &[StageRow],
        derived_from_seq: i64,
    ) -> Result<(), AppError> {
        for stage in stages {
            self.upsert_stage(stage, derived_from_seq)?;
        }
        Ok(())
    }

    pub fn read_stages(&self) -> Result<Vec<StageRow>, AppError> {
        self.with_read(|conn| {
            let mut links: std::collections::BTreeMap<String, Vec<(i64, String)>> =
                std::collections::BTreeMap::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT stage_id, activity_id, position FROM stage_activities
                     ORDER BY stage_id, position",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>("stage_id")?,
                        row.get::<_, i64>("position")?,
                        row.get::<_, String>("activity_id")?,
                    ))
                })?;
                for row in rows {
                    let (stage_id, position, activity_id) = row?;
                    links
                        .entry(stage_id)
                        .or_default()
                        .push((position, activity_id));
                }
            }

            let mut stmt = conn.prepare(
                "SELECT id, start_utc_ms, end_utc_ms, state, end_reason, day
                 FROM stages ORDER BY start_utc_ms, id",
            )?;
            let rows = stmt.query_map([], |row| {
                let id: String = row.get("id")?;
                let activities = links
                    .get(&id)
                    .map(|items| items.iter().map(|(_, id)| id.clone()).collect())
                    .unwrap_or_default();

                Ok(StageRow {
                    id,
                    start: Timestamp::from_millis(row.get::<_, i64>("start_utc_ms")?),
                    end: row
                        .get::<_, Option<i64>>("end_utc_ms")?
                        .map(Timestamp::from_millis),
                    state: row.get("state")?,
                    end_reason: row.get("end_reason")?,
                    day: row.get("day")?,
                    activities,
                })
            })?;

            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// **巡检的核心查询**：已关闭、超过 deadline、却没有任何总结的阶段。
    ///
    /// 「已关闭」是前提：还开着的阶段不该被催总结（它还没结束）。
    pub fn stages_missing_summary(
        &self,
        now: Timestamp,
        deadline_secs: u64,
    ) -> Result<Vec<StageRow>, AppError> {
        let cutoff = now.as_millis() - deadline_secs as i64 * 1000;
        let all = self.read_stages()?;
        let with_summary = self.stage_ids_with_summary()?;

        Ok(all
            .into_iter()
            .filter(|stage| stage.is_closed())
            .filter(|stage| {
                stage
                    .end
                    .map(|end| end.as_millis() <= cutoff)
                    .unwrap_or(false)
            })
            .filter(|stage| !with_summary.contains(&stage.id))
            .collect())
    }

    /// **哨兵指标**：`state=closed`、**本应有总结**却没有的阶段数。
    ///
    /// `min_duration_secs` 不是可选项：极短阶段本就不该产出总结
    /// （否则防抖残留会变成噪声总结）。若不把这类阶段排除在外，
    /// 指标就永远不可能为 0 —— 一个永远报警的指标等于没有指标。
    ///
    /// 因此这里量的是「**该有而没有**」，而不是「没有」。
    pub fn stages_without_summary_count(&self, min_duration_secs: u64) -> Result<i64, AppError> {
        let min_ms = min_duration_secs as i64 * 1000;
        self.with_read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM stages s
                 WHERE s.state = 'closed'
                   AND COALESCE(s.end_utc_ms, s.start_utc_ms) - s.start_utc_ms >= ?1
                   AND NOT EXISTS (SELECT 1 FROM summaries m WHERE m.stage_id = s.id)",
                rusqlite::params![min_ms],
                |row| row.get::<_, i64>(0),
            )
        })
    }

    pub fn stage_ids_with_summary(&self) -> Result<Vec<String>, AppError> {
        self.with_read(|conn| {
            let mut stmt =
                conn.prepare("SELECT DISTINCT stage_id FROM summaries WHERE stage_id IS NOT NULL")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }
}
