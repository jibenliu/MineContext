//! 用量预算闸门。
//!
//! 要防的形态是「2 小时 300 万 token，而 UI 没有明显产出」。
//! 预算是**硬闸门**：超限后不再调用模型，但**廉价元数据仍然记录** ——
//! 降级不是停摆。
//!
//! 用**滑动窗口**（60 个分钟桶 + 24 个小时桶）而不是自然小时/自然日：
//! 固定窗口允许在边界处突发放量，而突发正是我们要防的。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const MINUTE_BUCKETS: usize = 60;
const HOUR_BUCKETS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetAction {
    /// 降级到只做廉价处理（默认）
    Degrade,
    /// 完全暂停 AI
    Pause,
    /// 询问用户
    Ask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetPolicy {
    pub max_vlm_calls_per_hour: u32,
    pub max_tokens_per_hour: u64,
    pub max_tokens_per_day: u64,
    pub on_exceeded: BudgetAction,
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self {
            // 与 mc-config 的 ai.budget 默认值保持一致
            max_vlm_calls_per_hour: 240,
            max_tokens_per_hour: 400_000,
            max_tokens_per_day: 2_000_000,
            on_exceeded: BudgetAction::Degrade,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetEvent {
    Exceeded { scope: String },
    Recovered { scope: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetState {
    pub exceeded: bool,
    /// 超出的是哪一项（`hourly_calls` / `hourly_tokens` / `daily_tokens`）
    pub reason: Option<String>,
}

impl BudgetState {
    fn ok() -> Self {
        Self {
            exceeded: false,
            reason: None,
        }
    }

    fn exceeded_by(scope: &str) -> Self {
        Self {
            exceeded: true,
            reason: Some(scope.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetUsage {
    pub calls_last_hour: u32,
    pub tokens_last_hour: u64,
    pub tokens_last_day: u64,
    pub hourly_call_limit: u32,
    pub hourly_token_limit: u64,
    pub daily_token_limit: u64,
}

#[derive(Debug, Clone, Copy)]
struct MinuteBucket {
    minute: i64,
    calls: u32,
    tokens: u64,
}

#[derive(Debug, Clone, Copy)]
struct HourBucket {
    hour: i64,
    tokens: u64,
}

pub struct BudgetTracker {
    policy: BudgetPolicy,
    minutes: [MinuteBucket; MINUTE_BUCKETS],
    hours: [HourBucket; HOUR_BUCKETS],
    last_state: BudgetState,
}

impl BudgetTracker {
    pub fn new(policy: BudgetPolicy) -> Self {
        Self {
            policy,
            minutes: [MinuteBucket {
                minute: i64::MIN,
                calls: 0,
                tokens: 0,
            }; MINUTE_BUCKETS],
            hours: [HourBucket {
                hour: i64::MIN,
                tokens: 0,
            }; HOUR_BUCKETS],
            last_state: BudgetState::ok(),
        }
    }

    pub fn policy(&self) -> &BudgetPolicy {
        &self.policy
    }

    /// 记一次模型调用。返回状态**跃迁**事件（仅跃迁时返回，避免 UI 被刷屏）。
    pub fn record_call(&mut self, tokens: u64, at: Timestamp) -> Option<BudgetEvent> {
        let before = self.state(at);
        self.add(tokens, at);
        self.transition(before, self.state(at))
    }

    /// 不产生新调用，只探测窗口是否已经滑过。
    ///
    /// 「恢复」只能在**没有新调用**时被观测到 —— 一次新的调用本身就会再用掉额度。
    pub fn tick(&mut self, at: Timestamp) -> Option<BudgetEvent> {
        let before = self.last_state.clone();
        self.transition(before, self.state(at))
    }

    pub fn state(&self, at: Timestamp) -> BudgetState {
        let minute = at.as_millis() / MINUTE_MS;
        let (calls, tokens) = self.window_sums(minute);
        let day_tokens = self.day_sum(at.as_millis() / HOUR_MS);

        if calls >= self.policy.max_vlm_calls_per_hour {
            return BudgetState::exceeded_by("hourly_calls");
        }
        if tokens >= self.policy.max_tokens_per_hour {
            return BudgetState::exceeded_by("hourly_tokens");
        }
        if day_tokens >= self.policy.max_tokens_per_day {
            return BudgetState::exceeded_by("daily_tokens");
        }

        BudgetState::ok()
    }

    pub fn usage(&self, at: Timestamp) -> BudgetUsage {
        let minute = at.as_millis() / MINUTE_MS;
        let (calls, tokens) = self.window_sums(minute);
        let day_tokens = self.day_sum(at.as_millis() / HOUR_MS);

        BudgetUsage {
            calls_last_hour: calls,
            tokens_last_hour: tokens,
            tokens_last_day: day_tokens,
            hourly_call_limit: self.policy.max_vlm_calls_per_hour,
            hourly_token_limit: self.policy.max_tokens_per_hour,
            daily_token_limit: self.policy.max_tokens_per_day,
        }
    }

    fn transition(&mut self, before: BudgetState, after: BudgetState) -> Option<BudgetEvent> {
        let event = match (&before.exceeded, &after.exceeded) {
            (false, true) => Some(BudgetEvent::Exceeded {
                scope: after.reason.clone().unwrap_or_default(),
            }),
            (true, false) => Some(BudgetEvent::Recovered {
                scope: before.reason.clone().unwrap_or_default(),
            }),
            _ => None,
        };
        self.last_state = after;
        event
    }

    fn add(&mut self, tokens: u64, at: Timestamp) {
        let minute = at.as_millis() / MINUTE_MS;
        let index = (minute.rem_euclid(MINUTE_BUCKETS as i64)) as usize;
        let bucket = &mut self.minutes[index];
        if bucket.minute != minute {
            *bucket = MinuteBucket {
                minute,
                calls: 0,
                tokens: 0,
            };
        }
        bucket.calls += 1;
        bucket.tokens += tokens;

        let hour = at.as_millis() / HOUR_MS;
        let index = (hour.rem_euclid(HOUR_BUCKETS as i64)) as usize;
        let bucket = &mut self.hours[index];
        if bucket.hour != hour {
            *bucket = HourBucket { hour, tokens: 0 };
        }
        bucket.tokens += tokens;
    }

    /// 滑动小时窗口：只统计落在 `[minute-59, minute]` 内的桶。
    fn window_sums(&self, current_minute: i64) -> (u32, u64) {
        let mut calls = 0u32;
        let mut tokens = 0u64;
        for bucket in &self.minutes {
            // 必须先判空桶再算差，否则 `current - i64::MIN` 会溢出
            if bucket.minute == i64::MIN {
                continue;
            }
            let age = current_minute - bucket.minute;
            if (0..MINUTE_BUCKETS as i64).contains(&age) {
                calls += bucket.calls;
                tokens += bucket.tokens;
            }
        }
        (calls, tokens)
    }

    /// 滑动 24 小时窗口。
    fn day_sum(&self, current_hour: i64) -> u64 {
        let mut tokens = 0u64;
        for bucket in &self.hours {
            if bucket.hour == i64::MIN {
                continue;
            }
            let age = current_hour - bucket.hour;
            if (0..HOUR_BUCKETS as i64).contains(&age) {
                tokens += bucket.tokens;
            }
        }
        tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_slides_rather_than_resetting_on_the_hour() {
        // 滑动窗口的意义：23:59 用满额度后，00:00 不会立刻又满血复活
        let policy = BudgetPolicy {
            max_vlm_calls_per_hour: 2,
            max_tokens_per_hour: u64::MAX,
            max_tokens_per_day: u64::MAX,
            on_exceeded: BudgetAction::Degrade,
        };
        let mut tracker = BudgetTracker::new(policy);

        let almost_midnight = 1_790_758_740_000i64; // 任意时刻
        tracker.record_call(0, Timestamp::from_millis(almost_midnight));
        tracker.record_call(0, Timestamp::from_millis(almost_midnight));

        // 1 分钟后仍然超限（固定窗口会在这里错误地恢复）
        let one_minute_later = Timestamp::from_millis(almost_midnight + MINUTE_MS);
        assert!(
            tracker.state(one_minute_later).exceeded,
            "滑动窗口不应在整点边界重置"
        );
    }

    #[test]
    fn stale_buckets_are_not_counted() {
        let policy = BudgetPolicy {
            max_vlm_calls_per_hour: 1,
            max_tokens_per_hour: u64::MAX,
            max_tokens_per_day: u64::MAX,
            on_exceeded: BudgetAction::Degrade,
        };
        let mut tracker = BudgetTracker::new(policy);
        let start = Timestamp::from_millis(1_000_000_000_000);

        tracker.record_call(0, start);
        assert!(tracker.state(start).exceeded);

        // 两小时后同一个槽位被复用，旧数据必须被清掉
        let much_later = Timestamp::from_millis(start.as_millis() + 2 * HOUR_MS);
        assert!(
            !tracker.state(much_later).exceeded,
            "旧桶必须被清理，否则计数器会永久卡住"
        );
        assert_eq!(tracker.usage(much_later).calls_last_hour, 0);
    }
}
