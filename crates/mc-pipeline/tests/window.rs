//! 录制时段判定。
//!
//! `enableRecordingHours` 打开时按「只工作日」+「HH:mm–HH:mm」判断能否采集。
//! 朴素写法用「今天的日期 + 起止时间」拼出两个时刻再比 `now`，跨夜分支因此坏掉：
//! `now >= start || now <= end` 几乎恒为真，跨夜时段等于永不生效。
//!
//! 这里做成纯函数并按正确语义实现，因此所有边界都能穷举测试。

use mc_pipeline::window::{CaptureWindow, WindowDecision};
use mc_testkit::fixtures::FIXTURE_EPOCH_MS;

// ---------------------------------------------------------------- 解析

#[test]
fn parses_hh_mm() {
    let window = CaptureWindow::parse(false, "09:00", "18:00").expect("合法时段");
    assert_eq!(window.start_minute(), 9 * 60);
    assert_eq!(window.end_minute(), 18 * 60);
}

/// 前端 `recordingHours` 实际发的是 `HH:mm:ss`
/// （`store/setting.ts` 默认值就是 `['08:00:00','20:00:00']`），必须能解析。
#[test]
fn accepts_hh_mm_ss_as_sent_by_the_ui() {
    let window = CaptureWindow::parse(true, "08:00:00", "20:00:00").expect("UI 实际发的格式");
    assert_eq!(window.start_minute(), 8 * 60);
    assert_eq!(window.end_minute(), 20 * 60);
    assert_eq!(
        window.decide_weekday(true, 12 * 60),
        WindowDecision::Allowed
    );
    assert_eq!(
        window.decide_weekday(true, 21 * 60),
        WindowDecision::Outside
    );
}

#[test]
fn rejects_malformed_times_without_restricting_capture() {
    // 的选择是「格式不合法就跳过检查」→ 不限制。
    // 配置写错不该让用户「以为设了时段，实际一直不采集」。
    for (start, end) in [
        ("9", "18:00"),
        ("09:00", "18"),
        ("aa:bb", "18:00"),
        ("24:00", "18:00"),
        ("08:00:xx", "20:00:00"),
    ] {
        let window = CaptureWindow::parse(false, start, end);
        assert!(
            window.is_none(),
            "{start}-{end} 应当被判定为无效（从而不施加限制）"
        );
    }
}

// ---------------------------------------------------------------- 时段

#[test]
fn same_day_window_is_inclusive_on_both_ends() {
    let window = CaptureWindow::parse(false, "09:00", "18:00").unwrap();

    assert_eq!(
        window.decide_weekday(true, 8 * 60 + 59),
        WindowDecision::Outside
    );
    assert_eq!(window.decide_weekday(true, 9 * 60), WindowDecision::Allowed);
    assert_eq!(
        window.decide_weekday(true, 12 * 60),
        WindowDecision::Allowed
    );
    assert_eq!(
        window.decide_weekday(true, 18 * 60),
        WindowDecision::Allowed
    );
    assert_eq!(
        window.decide_weekday(true, 18 * 60 + 1),
        WindowDecision::Outside
    );
}

/// 跨夜时段（22:00–06:00）必须真的生效 —— 这正是坏掉的分支。
#[test]
fn overnight_window_wraps_around_midnight() {
    let window = CaptureWindow::parse(false, "22:00", "06:00").unwrap();

    assert_eq!(
        window.decide_weekday(true, 23 * 60),
        WindowDecision::Allowed
    );
    assert_eq!(window.decide_weekday(true, 0), WindowDecision::Allowed);
    assert_eq!(
        window.decide_weekday(true, 5 * 60 + 59),
        WindowDecision::Allowed
    );
    assert_eq!(window.decide_weekday(true, 6 * 60), WindowDecision::Allowed);
    assert_eq!(
        window.decide_weekday(true, 6 * 60 + 1),
        WindowDecision::Outside
    );
    assert_eq!(
        window.decide_weekday(true, 12 * 60),
        WindowDecision::Outside
    );
}

/// 起止相同 = 全天（不是「零长度」）：用户设 00:00–00:00 的意图是「不限时段」。
#[test]
fn identical_bounds_mean_all_day() {
    let window = CaptureWindow::parse(false, "00:00", "00:00").unwrap();
    assert_eq!(window.decide_weekday(true, 0), WindowDecision::Allowed);
    assert_eq!(
        window.decide_weekday(true, 13 * 60),
        WindowDecision::Allowed
    );
    assert_eq!(
        window.decide_weekday(true, 23 * 60 + 59),
        WindowDecision::Allowed
    );
}

// ---------------------------------------------------------------- 工作日

#[test]
fn weekdays_only_skips_the_weekend() {
    let window = CaptureWindow::parse(true, "09:00", "18:00").unwrap();

    assert_eq!(
        window.decide_weekday(true, 10 * 60),
        WindowDecision::Allowed
    );
    assert_eq!(
        window.decide_weekday(false, 10 * 60),
        WindowDecision::Weekend,
        "周末必须单独给出原因（诊断里要能解释「为什么没采集」）"
    );
}

/// 不限制工作日时，周末照常采集
#[test]
fn weekend_is_allowed_when_weekdays_only_is_off() {
    let window = CaptureWindow::parse(false, "09:00", "18:00").unwrap();
    assert_eq!(
        window.decide_weekday(false, 10 * 60),
        WindowDecision::Allowed
    );
}

// ---------------------------------------------------------------- 时区

/// 判定必须按**本地时间**：UTC 03:00 在 Asia/Shanghai 是 11:00。
#[test]
fn decision_uses_the_configured_timezone() {
    let window = CaptureWindow::parse(false, "09:00", "18:00").unwrap();

    // 2026-09-30T03:00:00Z = 上海 11:00（周三）
    let at = mc_common::time::Timestamp::from_millis(FIXTURE_EPOCH_MS - 6 * 3_600_000);
    assert_eq!(
        window.decide_at(at, "Asia/Shanghai"),
        WindowDecision::Allowed
    );

    // 同一时刻在 UTC 是 03:00 → 不在时段内
    assert_eq!(window.decide_at(at, "UTC"), WindowDecision::Outside);
}

/// 时区解析失败时**不限制**（宁可多采集，也不要因为配置错误静默停采）
#[test]
fn unknown_timezone_does_not_block_capture() {
    let window = CaptureWindow::parse(false, "09:00", "18:00").unwrap();
    let at = mc_common::time::Timestamp::from_millis(FIXTURE_EPOCH_MS);
    assert_eq!(window.decide_at(at, "Not/AZone"), WindowDecision::Allowed);
}

#[test]
fn decision_has_a_human_readable_reason() {
    let window = CaptureWindow::parse(true, "09:00", "18:00").unwrap();
    let weekend = window.decide_weekday(false, 10 * 60);
    assert!(!weekend.is_allowed());
    let message = weekend.reason(false);
    assert!(
        message.contains("周末") || message.contains("工作日"),
        "原因要能直接展示：{message}"
    );
}
