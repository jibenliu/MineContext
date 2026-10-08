//! 证明「时间可注入」这条工程前提真的可用。
//!
//! `TestClock` 是后续所有时间敏感测试（Stage 状态机、防抖、空闲判定、
//! 跨天边界、退避重试）的基础——没有它，TDD 在这个领域就只能靠 sleep。

use mc_common::time::{Clock, Timestamp};
use mc_testkit::TestClock;

#[test]
fn test_clock_starts_at_given_instant() {
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    assert_eq!(
        clock.now(),
        Timestamp::parse_rfc3339("2026-09-30T09:00:00Z").unwrap()
    );
}

#[test]
fn test_clock_advances_deterministically() {
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    clock.advance(std::time::Duration::from_secs(45));
    assert_eq!(
        clock.now(),
        Timestamp::parse_rfc3339("2026-09-30T09:00:45Z").unwrap()
    );

    clock.advance(std::time::Duration::from_secs(15));
    assert_eq!(
        clock.now(),
        Timestamp::parse_rfc3339("2026-09-30T09:01:00Z").unwrap()
    );
}

#[test]
fn test_clock_can_be_set_backwards_for_clock_anomaly_tests() {
    // 系统时钟回拨是真实场景，必须可测
    let clock = TestClock::at("2026-09-30T09:00:00Z");

    clock.set(Timestamp::parse_rfc3339("2026-09-30T08:00:00Z").unwrap());

    assert_eq!(
        clock.now(),
        Timestamp::parse_rfc3339("2026-09-30T08:00:00Z").unwrap()
    );
}

#[test]
fn test_clock_is_shareable_across_tasks() {
    let clock = TestClock::at("2026-09-30T09:00:00Z");
    let handle = clock.clone();

    clock.advance(std::time::Duration::from_secs(60));

    assert_eq!(handle.now(), clock.now());
}
