//! 时间类型。
//!
//! 时区混用是这里最贵的一类缺陷：naive 与 timezone-aware datetime 混用会直接抛
//! `TypeError: can't compare offset-naive and offset-aware datetimes`。
//! 这些测试是它的第一道防线。

use mc_common::time::{Clock, Timestamp};

/// 测试用的最小 Clock —— 只为了证明「时间可注入」这条工程前提成立。
struct FixedClock(Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

fn ts(rfc3339: &str) -> Timestamp {
    Timestamp::parse_rfc3339(rfc3339).expect("test fixture must parse")
}

#[test]
fn timestamp_is_utc_millis_from_clock() {
    let clock = FixedClock(Timestamp::from_millis(1_756_000_000_000));

    assert_eq!(Timestamp::now(&clock).as_millis(), 1_756_000_000_000);
}

#[test]
fn timestamp_orders_monotonically() {
    let a = ts("2026-09-30T09:00:00Z");
    let b = ts("2026-09-30T09:00:01Z");

    assert!(a < b);
    assert_eq!(b.as_millis() - a.as_millis(), 1_000);
}

// 0.2b
#[test]
fn timestamp_roundtrips_rfc3339() {
    let t = ts("2026-09-30T09:00:00Z");
    let again = Timestamp::parse_rfc3339(&t.to_rfc3339()).expect("roundtrip must parse");

    assert_eq!(t, again);
}

// 0.2c — 同一个 UTC 瞬间，在不同时区属于不同「本地日期」
#[test]
fn local_date_respects_iana_timezone() {
    let t = ts("2026-09-30T16:30:00Z");

    assert_eq!(
        t.to_local_date("Asia/Shanghai").unwrap().to_string(),
        "2026-10-01"
    );
    assert_eq!(t.to_local_date("UTC").unwrap().to_string(), "2026-09-30");
}

// 0.3 — DST 春令时（跳过 1 小时）：本地日只有 23 小时
#[test]
fn day_boundary_across_dst_spring_forward_is_23h() {
    let tz = "America/New_York";
    let (start, end) = ts("2026-03-08T12:00:00Z")
        .day_bounds(tz)
        .expect("day bounds must compute");

    assert!(start < end);
    assert_eq!(end.as_millis() - start.as_millis(), 23 * 3_600_000);
    assert_eq!(start.to_local_date(tz).unwrap().to_string(), "2026-03-08");
}

// 0.3b — DST 冬令时（重复 1 小时）：本地日有 25 小时
#[test]
fn day_boundary_across_dst_fall_back_is_25h() {
    let tz = "America/New_York";
    let (start, end) = ts("2026-11-01T12:00:00Z")
        .day_bounds(tz)
        .expect("day bounds must compute");

    assert_eq!(end.as_millis() - start.as_millis(), 25 * 3_600_000);
    assert_eq!(start.to_local_date(tz).unwrap().to_string(), "2026-11-01");
}

// 0.3c — 本地午夜根本不存在（Santiago 的 DST 切换发生在 00:00）
// 期望：不 panic、边界有序、仍落在请求的那一天。
#[test]
fn day_boundary_with_nonexistent_local_midnight_does_not_panic() {
    let tz = "America/Santiago";
    let (start, end) = ts("2026-09-06T12:00:00Z")
        .day_bounds(tz)
        .expect("day bounds must compute");

    assert!(start < end, "start={start:?} end={end:?}");
    let span = end.as_millis() - start.as_millis();
    assert!(
        (22 * 3_600_000..=25 * 3_600_000).contains(&span),
        "unexpected span: {span} ms"
    );
    assert_eq!(start.to_local_date(tz).unwrap().to_string(), "2026-09-06");
}

// 0.3d — 无 DST 的时区应当恰好 24 小时
#[test]
fn day_boundary_without_dst_is_exactly_24h() {
    let tz = "Asia/Shanghai";
    let (start, end) = ts("2026-09-30T12:00:00Z")
        .day_bounds(tz)
        .expect("day bounds must compute");

    assert_eq!(end.as_millis() - start.as_millis(), 24 * 3_600_000);
    assert_eq!(start.to_local_date(tz).unwrap().to_string(), "2026-09-30");
}

// 0.3e — 未知时区必须报错，而不是静默回退到 UTC（静默回退是数据损坏的来源）
#[test]
fn unknown_timezone_is_an_error_not_a_silent_fallback() {
    let t = ts("2026-09-30T12:00:00Z");
    let err = t.to_local_date("Not/AZone").unwrap_err();

    assert_eq!(
        err.code(),
        mc_common::error::ErrorCode::ConfigUnknownTimezone
    );
}

// 0.4 — 相邻两天的边界必须首尾相接（不能有空洞或重叠）
#[test]
fn consecutive_day_bounds_are_contiguous() {
    let tz = "America/New_York";
    let (_, end_day1) = ts("2026-03-08T12:00:00Z").day_bounds(tz).unwrap();
    let (start_day2, _) = ts("2026-03-09T12:00:00Z").day_bounds(tz).unwrap();

    assert_eq!(end_day1, start_day2);
}

// 展示用本地时间：写错时区会让总结里的时间段与用户看到的不一致
#[test]
fn formats_local_time_in_the_configured_timezone() {
    // 2026-09-30T09:00:00Z = 北京时间 17:00
    let at = Timestamp::parse_rfc3339("2026-09-30T09:00:00Z").unwrap();

    assert_eq!(at.format_in_tz("Asia/Shanghai", "%H:%M").unwrap(), "17:00");
    assert_eq!(at.format_in_tz("UTC", "%H:%M").unwrap(), "09:00");
    // DST 生效期间（纽约夏令时 UTC-4）
    assert_eq!(
        at.format_in_tz("America/New_York", "%H:%M").unwrap(),
        "05:00"
    );

    let error = at
        .format_in_tz("Not/AZone", "%H:%M")
        .expect_err("未知时区必须报错");
    assert_eq!(
        error.code(),
        mc_common::error::ErrorCode::ConfigUnknownTimezone
    );
}

// 按日期取日边界：DST 日不是 24 小时，不能用 now - 24h 算「昨天」
#[test]
fn day_bounds_for_date_handles_dst() {
    // 美国夏令时结束当天（2026-11-01，America/New_York）有 25 小时
    let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let (start, end) = Timestamp::day_bounds_for(date, "America/New_York").unwrap();

    let hours = (end.as_millis() - start.as_millis()) as f64 / 3_600_000.0;
    assert_eq!(hours, 25.0, "DST 结束当天应当有 25 小时，实际 {hours}");

    // 春令时开始当天（2026-03-08）只有 23 小时
    let date = chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
    let (start, end) = Timestamp::day_bounds_for(date, "America/New_York").unwrap();
    let hours = (end.as_millis() - start.as_millis()) as f64 / 3_600_000.0;
    assert_eq!(hours, 23.0, "DST 开始当天应当有 23 小时，实际 {hours}");

    // 固定偏移的时区就是 24 小时
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
    let (start, end) = Timestamp::day_bounds_for(date, "Asia/Shanghai").unwrap();
    assert_eq!((end.as_millis() - start.as_millis()) / 3_600_000, 24);
}
