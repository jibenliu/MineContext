//! 录制时段：只在工作日的某个时间段内采集。
//!
//! 跨夜时段（如 22:00–06:00）不能按「今天的日期 + 起止时间」拼出两个时刻再比
//! 较：`now >= start || now <= end` 几乎恒为真，跨夜时段等于永不生效。
//! 这里按**意图**写成纯函数，因此每个边界都能穷举测试。
//!
//! 判定一律基于**配置时区下的本地时间**：用户写「09:00–18:00」指的是他所在
//! 时区的 09:00，而不是 UTC 的。

use mc_common::time::Timestamp;

/// 判定结果。周末与时段外分开，是为了让诊断能解释「为什么没采集」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowDecision {
    Allowed,
    Outside,
    Weekend,
}

impl WindowDecision {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// 给人看的一句话（诊断页与日志用）。
    pub fn reason(self, weekdays_only: bool) -> String {
        match self {
            Self::Allowed => "在录制时段内".to_string(),
            Self::Outside => "不在录制时段内".to_string(),
            Self::Weekend if weekdays_only => "工作日之外（当前仅工作日采集）".to_string(),
            Self::Weekend => "周末".to_string(),
        }
    }
}

/// 一个录制时段。以「当天的第几分钟」表示，避免引入时间类型的比较分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureWindow {
    weekdays_only: bool,
    start_minute: u32,
    end_minute: u32,
}

impl CaptureWindow {
    /// 解析 `HH:mm` 形式的起止时间。
    ///
    /// 不合法时返回 `None` —— 调用方据此**不施加限制**，
    /// 配置写错不该让用户遇到「以为设了时段，实际一直不采集」，
    /// 因此宁可放弃限制。
    pub fn parse(weekdays_only: bool, start: &str, end: &str) -> Option<Self> {
        Some(Self {
            weekdays_only,
            start_minute: parse_time_of_day(start)?,
            end_minute: parse_time_of_day(end)?,
        })
    }

    pub const fn start_minute(self) -> u32 {
        self.start_minute
    }

    pub const fn end_minute(self) -> u32 {
        self.end_minute
    }

    pub const fn weekdays_only(self) -> bool {
        self.weekdays_only
    }

    /// 纯判定：`is_weekday` 与「当天第几分钟」由调用方算好，便于穷举测试。
    pub fn decide_weekday(self, is_weekday: bool, minute_of_day: u32) -> WindowDecision {
        if self.weekdays_only && !is_weekday {
            return WindowDecision::Weekend;
        }
        if self.contains(minute_of_day) {
            WindowDecision::Allowed
        } else {
            WindowDecision::Outside
        }
    }

    /// 按配置时区判定某个时刻。
    ///
    /// 时区解析失败时返回 `Allowed`：宁可多采集，也不要因为一个配置笔误
    /// 静默停掉采集 —— 那是最难排查的一类问题。
    pub fn decide_at(self, at: Timestamp, timezone: &str) -> WindowDecision {
        let Ok(text) = at.format_in_tz(timezone, "%H:%M") else {
            return WindowDecision::Allowed;
        };
        let Some(minute_of_day) = parse_time_of_day(&text) else {
            return WindowDecision::Allowed;
        };

        // `%u` 是 ISO 星期几（1=周一 … 7=周日），避免为此引入日期库依赖
        let is_weekday = at
            .format_in_tz(timezone, "%u")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|day| day <= 5)
            .unwrap_or(true);

        self.decide_weekday(is_weekday, minute_of_day)
    }

    /// 分钟是否落在窗口内（含两端）。
    ///
    /// - 起止相同 → 全天（用户设 `00:00–00:00` 的意图是「不限时段」）；
    /// - 起点晚于终点 → 跨夜（`22:00–06:00` 覆盖到次日凌晨）。
    fn contains(self, minute_of_day: u32) -> bool {
        if self.start_minute == self.end_minute {
            return true;
        }
        if self.start_minute < self.end_minute {
            minute_of_day >= self.start_minute && minute_of_day <= self.end_minute
        } else {
            minute_of_day >= self.start_minute || minute_of_day <= self.end_minute
        }
    }
}

/// 解析时间 → 当天第几分钟。接受 `HH:mm` 与 `HH:mm:ss`。
///
/// 秒是前端实际会发的格式（`store/setting.ts` 的默认值是 `08:00:00`），
/// 对分钟粒度的录制时段没有意义，因此读进来直接丢弃。
/// 但格式必须严格：`9:00` 这类写法语义含糊，明确拒绝而不是猜。
fn parse_time_of_day(text: &str) -> Option<u32> {
    let mut parts = text.trim().split(':');
    let hours = parts.next()?;
    let minutes = parts.next()?;

    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }

    if let Some(seconds) = parts.next() {
        if seconds.len() != 2 || parts.next().is_some() {
            return None;
        }
        // 只校验合法性，不参与判定（录像是分钟粒度）
        let seconds: u32 = seconds.parse().ok()?;
        if seconds > 59 {
            return None;
        }
    }

    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(hours * 60 + minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_values() {
        assert!(parse_time_of_day("24:00").is_none());
        assert!(parse_time_of_day("12:60").is_none());
        assert!(parse_time_of_day("12:5").is_none());
        assert_eq!(parse_time_of_day(" 07:05 "), Some(7 * 60 + 5));
    }
}
