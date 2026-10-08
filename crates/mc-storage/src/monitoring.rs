//! 录制统计的原始计数（兼容 `/api/monitoring/recording-stats` 的数据源）。
//!
//! 统计从库里现算，避免重启后归零 ——
//! 用户会看到「今天处理了 0 张截图」而磁盘上明明全是截图。
//! 这里所有数字都从库里算：库本来就是真源（观测、活动、失败、截图路径），
//! 再维护一份计数器只会多一个会漂移的数字。
//!
//! SQL 留在 storage 层，`mc-server` 只负责算 ETA 和拼线上形状。

use mc_common::error::AppError;
use rusqlite::OptionalExtension;

use crate::db::Database;

/// 一次会话内的原始计数。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordingCounts {
    /// 采到的截图张数（观测在采集时写入）
    pub captured_screenshots: i64,
    /// 已进入模型分析并出结果的张数（`analyses.status` 为 `done` / `degraded`）
    pub processed_screenshots: i64,
    pub failed_screenshots: i64,
    pub generated_activities: i64,
    /// 最近一次活动的结束时间（毫秒），没有活动时为 `None`
    pub last_activity_ms: Option<i64>,
    pub recent_errors: Vec<RecordingError>,
    /// 最近截图的**相对**路径（相对 blob 目录）
    pub recent_screenshot_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingError {
    pub message: String,
    pub component: String,
    pub at_ms: i64,
}

/// 统计「本次会话」（`since_ms` 之后）的计数。
///
/// 两个数分开给：`captured_screenshots` 是采到的张数（观测在**采集时**写入），
/// `processed_screenshots` 是其中分析出结果的条数（`analyses.status` 为
/// `done`/`degraded`；没配模型时就是 0，那是事实不是异常）。合成一个数会把
/// 「采到了」讲成「处理完了」。`failed_screenshots` 数采集与视觉两处的失败；
/// `generated_activities` 用结束时间判定，活动可能横跨 daemon 启动时刻。
pub fn recording_counts(
    db: &Database,
    since_ms: i64,
    capture_components: &[&str],
    limit: usize,
) -> Result<RecordingCounts, AppError> {
    db.with_read(|conn| {
        let captured_screenshots: i64 = conn.query_row(
            "SELECT COUNT(*) FROM observations WHERE kind = 'screen' AND ts_utc_ms >= ?1",
            [since_ms],
            |row| row.get(0),
        )?;

        let processed_screenshots: i64 = conn.query_row(
            "SELECT COUNT(*) FROM analyses a
               JOIN observations o ON o.id = a.observation_id
              WHERE o.kind = 'screen' AND o.ts_utc_ms >= ?1
                AND a.status IN ('done', 'degraded')",
            [since_ms],
            |row| row.get(0),
        )?;

        let placeholders = capture_components
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let failed_sql = format!(
            "SELECT COUNT(*) FROM pipeline_failures
              WHERE at_utc_ms >= ?1 AND component IN ({placeholders})"
        );
        let mut failed_params: Vec<rusqlite::types::Value> =
            vec![rusqlite::types::Value::Integer(since_ms)];
        for component in capture_components {
            failed_params.push(rusqlite::types::Value::Text((*component).to_string()));
        }
        let failed_screenshots: i64 = conn.query_row(
            &failed_sql,
            rusqlite::params_from_iter(failed_params.iter()),
            |row| row.get(0),
        )?;

        let generated_activities: i64 = conn.query_row(
            "SELECT COUNT(*) FROM activities WHERE COALESCE(end_utc_ms, start_utc_ms) >= ?1",
            [since_ms],
            |row| row.get(0),
        )?;

        let last_activity_ms: Option<i64> = conn
            .query_row(
                "SELECT COALESCE(end_utc_ms, start_utc_ms) FROM activities
                  ORDER BY COALESCE(end_utc_ms, start_utc_ms) DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;

        let recent_errors = {
            let mut stmt = conn.prepare(
                "SELECT message, component, at_utc_ms FROM pipeline_failures
                  WHERE at_utc_ms >= ?1
                  ORDER BY at_utc_ms DESC, id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![since_ms, limit as i64], |row| {
                Ok(RecordingError {
                    message: row.get(0)?,
                    component: row.get(1)?,
                    at_ms: row.get(2)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let recent_screenshot_paths = {
            let mut stmt = conn.prepare(
                "SELECT image_path FROM observations
                  WHERE kind = 'screen' AND ts_utc_ms >= ?1 AND image_path IS NOT NULL
                  ORDER BY ts_utc_ms DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![since_ms, limit as i64], |row| {
                row.get::<_, String>(0)
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        Ok(RecordingCounts {
            captured_screenshots,
            processed_screenshots,
            failed_screenshots,
            generated_activities,
            last_activity_ms,
            recent_errors,
            recent_screenshot_paths,
        })
    })
}

/// 统计失败（读真实的失败记录）。
pub fn recent_failures(
    db: &Database,
    since_ms: i64,
    limit: usize,
) -> Result<Vec<RecordingError>, AppError> {
    db.with_read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT message, component, at_utc_ms FROM pipeline_failures
              WHERE at_utc_ms >= ?1
              ORDER BY at_utc_ms DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![since_ms, limit as i64], |row| {
            Ok(RecordingError {
                message: row.get(0)?,
                component: row.get(1)?,
                at_ms: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}
