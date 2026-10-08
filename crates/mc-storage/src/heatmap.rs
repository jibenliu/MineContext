//! 热力图聚合（`heatmap:get-data` 的数据源）。
//!
//! 按**本地日**分桶四类数据（已完成任务、会话、笔记、截图/文档/上下文计数）。
//! 兼容 `data_type` 的映射：`screenshot` → `observations.kind='screen'`、
//! `document` → `'file'`、其余 kind（`window`/`clipboard`/`browser`）→ `context`。
//!
//! 两条必须照做的兼容细节：**每一天都要有一行**（含零值），否则前端按日期做
//! 格子会出现空洞；**时间列格式每张表都不同**（ISO / `YYYY-MM-DD HH:MM:SS` /
//! 整数毫秒），边界必须按各自格式生成 —— 否则比较静默为假（`T` 的字典序大于
//! 空格），表现为「什么都数不到」。

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;

use crate::db::Database;

/// 一天的分项计数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayCounts {
    /// `YYYY-MM-DD`（本地日）
    pub date: String,
    pub todos: i64,
    pub conversations: i64,
    pub vaults: i64,
    pub screenshots: i64,
    pub documents: i64,
    pub contexts: i64,
}

impl DayCounts {
    pub const fn total(&self) -> i64 {
        self.todos
            + self.conversations
            + self.vaults
            + self.screenshots
            + self.documents
            + self.contexts
    }
}

/// 允许的最大天数。
///
/// 区间内每一天建对象是 O(天数) 的内存成本；「1970 → 2999」这类区间能让主进程
/// 吃满内存。366 天覆盖热力图的所有视图（月/年），超出直接报错。
pub const MAX_HEATMAP_DAYS: i64 = 366;

/// 按本地日聚合 `[start, end]` 区间（含首尾两天）。
pub fn heatmap_counts(
    db: &Database,
    start: Timestamp,
    end: Timestamp,
    timezone: &str,
) -> Result<Vec<DayCounts>, AppError> {
    if start > end {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!(
                "热力图的时间范围反了：start={} 晚于 end={}",
                start.to_rfc3339(),
                end.to_rfc3339()
            ),
        ));
    }

    let first = start.to_local_date(timezone)?;
    let last = end.to_local_date(timezone)?;
    let days = (last - first).num_days() + 1;
    if days > MAX_HEATMAP_DAYS {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("热力图范围过大：{days} 天，上限 {MAX_HEATMAP_DAYS} 天"),
        ));
    }

    let mut result = Vec::with_capacity(days as usize);
    let mut date = first;
    while date <= last {
        let (day_start, day_end) = Timestamp::day_bounds_for(date, timezone)?;

        // 各表的时间格式不同：分别生成边界文本
        let iso_start = day_start.to_rfc3339();
        let iso_end = day_end.to_rfc3339();
        let legacy_start = day_start.to_legacy_datetime();
        let legacy_end = day_end.to_legacy_datetime();
        let (ms_start, ms_end) = (day_start.as_millis(), day_end.as_millis());

        let counts = db.with_read(|conn| {
            let todos: i64 = conn.query_row(
                "SELECT COUNT(*) FROM todo
                  WHERE status = 1 AND start_time >= ?1 AND start_time < ?2",
                rusqlite::params![iso_start, iso_end],
                |row| row.get(0),
            )?;
            let conversations: i64 = conn.query_row(
                "SELECT COUNT(*) FROM conversations
                  WHERE created_at >= ?1 AND created_at < ?2",
                rusqlite::params![legacy_start, legacy_end],
                |row| row.get(0),
            )?;
            let vaults: i64 = conn.query_row(
                "SELECT COUNT(*) FROM vaults
                  WHERE created_at >= ?1 AND created_at < ?2",
                rusqlite::params![legacy_start, legacy_end],
                |row| row.get(0),
            )?;
            let screenshots: i64 = conn.query_row(
                "SELECT COUNT(*) FROM observations
                  WHERE kind = 'screen' AND ts_utc_ms >= ?1 AND ts_utc_ms < ?2",
                rusqlite::params![ms_start, ms_end],
                |row| row.get(0),
            )?;
            let documents: i64 = conn.query_row(
                "SELECT COUNT(*) FROM observations
                  WHERE kind = 'file' AND ts_utc_ms >= ?1 AND ts_utc_ms < ?2",
                rusqlite::params![ms_start, ms_end],
                |row| row.get(0),
            )?;
            let contexts: i64 = conn.query_row(
                "SELECT COUNT(*) FROM observations
                  WHERE kind NOT IN ('screen', 'file') AND ts_utc_ms >= ?1 AND ts_utc_ms < ?2",
                rusqlite::params![ms_start, ms_end],
                |row| row.get(0),
            )?;
            Ok((
                todos,
                conversations,
                vaults,
                screenshots,
                documents,
                contexts,
            ))
        })?;

        result.push(DayCounts {
            date: date.format("%Y-%m-%d").to_string(),
            todos: counts.0,
            conversations: counts.1,
            vaults: counts.2,
            screenshots: counts.3,
            documents: counts.4,
            contexts: counts.5,
        });

        date += chrono::Duration::days(1);
    }

    Ok(result)
}
