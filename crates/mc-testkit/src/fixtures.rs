//! 共享测试夹具：固定时间基准与示例对象构造器。
//!
//! 集成测试到处都要「一个确定的时刻」和「一条像样的活动」，此前这些值在 60 个
//! 测试文件里各写一份，于是改一处要改 60 处、同一个数字有六个名字、新测试只能
//! 靠复制粘贴起步。
//! [`FIXTURE_EPOCH_MS`] 是 **2026-09-30T09:00:00Z**：取固定值而不是 `now()`，
//! 是为了让时间断言、排序与日边界计算完全可复现（墙上时钟会让测试在跨天、
//! 跨时区、闰秒时随机失败）。示例文案用中文短句，是为了断言失败时的输出一眼
//! 能看懂。
//! 本 crate 是 `dev-dependencies`（`check-no-test-fixtures.sh` 会拦住这些值漏进 `src/**`）。

use mc_common::time::Timestamp;
use mc_domain::activity::{ActivityView, ObservationRef, Provenance};

/// 固定时间基准：**2026-09-30T09:00:00Z**（周三，UTC+8 的 17:00）。
pub const FIXTURE_EPOCH_MS: i64 = 1_790_758_800_000;

/// 一天（毫秒）。断言跨天行为时用它做偏移，比手写 86_400_000 更不容易错。
pub const FIXTURE_DAY_MS: i64 = 86_400_000;

/// 一小时（毫秒）。
pub const FIXTURE_HOUR_MS: i64 = 3_600_000;

/// 一分钟（毫秒）。
pub const FIXTURE_MINUTE_MS: i64 = 60_000;

/// 示例活动标题前缀。
pub const SAMPLE_ACTIVITY_TITLE: &str = "活动";

/// 示例活动分类。分类字段是自由文本，这里取一个真实会出现的值。
pub const SAMPLE_CATEGORY: &str = "开发";

/// 时间基准（`Timestamp` 形式）。
pub const fn fixture_epoch() -> Timestamp {
    Timestamp::from_millis(FIXTURE_EPOCH_MS)
}

/// 基准 + 偏移毫秒。
pub const fn fixture_at(offset_ms: i64) -> Timestamp {
    Timestamp::from_millis(FIXTURE_EPOCH_MS + offset_ms)
}

/// 基准 + N 分钟。
pub const fn fixture_minute(minutes: i64) -> Timestamp {
    fixture_at(minutes * FIXTURE_MINUTE_MS)
}

/// 基准 + N 小时。
pub const fn fixture_hour(hours: i64) -> Timestamp {
    fixture_at(hours * FIXTURE_HOUR_MS)
}

/// 基准 + N 天。
pub const fn fixture_day(days: i64) -> Timestamp {
    fixture_at(days * FIXTURE_DAY_MS)
}

/// 一条示例活动：标题 `活动 {id}`，分类 [`SAMPLE_CATEGORY`]，持续 5 分钟。
///
/// `ActivityView` 有十来个字段，绝大多数测试只关心标题与时间；
/// 集中一个构造器之后，字段增删只需要改这一处。
pub fn sample_activity(id: &str, at: Timestamp) -> ActivityView {
    sample_activity_with(id, at, &format!("{SAMPLE_ACTIVITY_TITLE} {id}"))
}

/// 同 [`sample_activity`]，但自己指定标题（需要实体、关键词等场景时用）。
pub fn sample_activity_with(id: &str, at: Timestamp, title: &str) -> ActivityView {
    ActivityView {
        id: id.to_string(),
        start: at,
        end: at.plus_millis(5 * FIXTURE_MINUTE_MS),
        title: title.to_string(),
        original_title: title.to_string(),
        category: Some(SAMPLE_CATEGORY.to_string()),
        observations: Vec::new(),
        origin: Provenance::Observed,
        confidence: 1.0,
        is_user_modified: false,
    }
}

/// 给活动补上证据观测（把 `observations` 与时间对齐）。
pub fn with_evidence(mut activity: ActivityView, observation_ids: &[&str]) -> ActivityView {
    activity.observations = observation_ids
        .iter()
        .map(|id| ObservationRef {
            id: (*id).to_string(),
            at: activity.start,
        })
        .collect();
    activity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_the_documented_instant() {
        // 2026-09-30T09:00:00Z
        assert_eq!(fixture_epoch().to_rfc3339(), "2026-09-30T09:00:00.000Z");
    }

    #[test]
    fn offsets_compose() {
        assert_eq!(
            fixture_day(1).as_millis(),
            FIXTURE_EPOCH_MS + FIXTURE_DAY_MS
        );
        assert_eq!(
            fixture_hour(2).as_millis(),
            FIXTURE_EPOCH_MS + 2 * FIXTURE_HOUR_MS
        );
        assert_eq!(
            fixture_minute(30).as_millis(),
            FIXTURE_EPOCH_MS + 30 * FIXTURE_MINUTE_MS
        );
    }

    #[test]
    fn sample_activity_is_self_consistent() {
        let activity = sample_activity("act-1", fixture_hour(1));
        assert_eq!(activity.title, "活动 act-1");
        assert_eq!(activity.original_title, activity.title);
        assert_eq!(activity.category.as_deref(), Some(SAMPLE_CATEGORY));
        assert_eq!(
            activity.end.as_millis() - activity.start.as_millis(),
            5 * FIXTURE_MINUTE_MS
        );
    }
}
