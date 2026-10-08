//! 前端传来的时间边界有**三种**形状，必须都能解析。
//!
//! - `heatmap:get-data` 传的是毫秒数（`dayjs().valueOf()`）；
//! - `database:get-all-tasks` 传的是 ISO 字符串（`dayjs().startOf('day').toISOString()`）；
//! - 旧库里存的是 `YYYY-MM-DD HH:MM:SS`（兼容层 DATETIME）。
//!
//! 只认其中一种的后果不是报错，而是**静默返回空结果**：
//! 首页的任务列表与热力图直接变空，而日志里什么都没有。

use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

#[test]
fn parses_epoch_millis() {
    let parsed = Timestamp::parse_flexible(&T0.to_string()).expect("毫秒数必须能解析");
    assert_eq!(parsed.as_millis(), T0);

    // 负数（1970 之前）也要能解析
    let before = Timestamp::parse_flexible("-1000").expect("负毫秒也必须能解析");
    assert_eq!(before.as_millis(), -1000);
}

#[test]
fn parses_rfc3339_with_and_without_millis() {
    let with_millis = Timestamp::parse_flexible("2026-09-30T09:00:00.000Z").expect("ISO（带毫秒）");
    let without = Timestamp::parse_flexible("2026-09-30T09:00:00Z").expect("ISO（不带毫秒）");
    let offset = Timestamp::parse_flexible("2026-09-30T17:00:00+08:00").expect("ISO（带偏移）");

    assert_eq!(with_millis.as_millis(), T0);
    assert_eq!(without.as_millis(), T0);
    assert_eq!(offset.as_millis(), T0);
}

#[test]
fn parses_legacy_datetime_as_utc() {
    let parsed = Timestamp::parse_flexible("2026-09-30 09:00:00").expect("兼容层格式");
    assert_eq!(parsed.as_millis(), T0);

    // 与自己的输出往返一致
    assert_eq!(
        Timestamp::parse_flexible(&Timestamp::from_millis(T0).to_legacy_datetime())
            .unwrap()
            .as_millis(),
        T0
    );
}

// 空白容忍：查询参数经常带着空格
#[test]
fn trims_whitespace() {
    assert_eq!(
        Timestamp::parse_flexible("  2026-09-30T09:00:00Z  ")
            .unwrap()
            .as_millis(),
        T0
    );
}

#[test]
fn rejects_garbage_with_a_stable_code() {
    for bad in ["", "  ", "昨天", "2026-13-45T99:00:00Z", "abc"] {
        let error = Timestamp::parse_flexible(bad).expect_err("必须拒绝：{bad}");
        assert_eq!(
            error.code(),
            ErrorCode::DomainInvalidTimestamp,
            "输入 {bad:?}"
        );
    }
}
