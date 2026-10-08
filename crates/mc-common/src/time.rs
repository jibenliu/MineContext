//! UTC 毫秒时间戳与 IANA 时区边界计算。
//!
//! 关键点：
//! - [`Timestamp`] 是 **UTC 毫秒** 的 newtype，是领域层唯一的时间表示。
//! - 没有任何 API 接受或返回 naive 时间（`NaiveDateTime`），因此
//!   「naive 与 aware 混比」在类型层面不可能发生。
//! - 时区只在 [`Timestamp::to_local_date`] / [`Timestamp::day_bounds`] 处使用，
//!   且必须显式传入 IANA 名称（未知时区**报错**，不静默回退 UTC —— 静默回退
//!   正是时区混用会造成 8 小时偏移的原因之一）。

use chrono::{DateTime, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, ErrorCode};

/// 可注入的时钟。所有需要「现在」的代码都必须走这里，
/// 测试用 `mc_testkit::TestClock` 注入受控时间。
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// 生产环境时钟。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp(Utc::now().timestamp_millis())
    }
}

/// UTC 毫秒时间戳（Unix epoch 起算）。
///
/// 序列化为裸 `i64`，便于直接落 SQLite INTEGER 列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

/// 时间边界解析失败的统一错误（细节里带上原文，便于定位是哪个参数）。
fn invalid_timestamp(raw: &str) -> AppError {
    AppError::new(
        ErrorCode::DomainInvalidTimestamp,
        format!(
            "无法解析时间 {raw:?}；支持毫秒数、RFC 3339（2026-09-30T09:00:00Z）\
             与 YYYY-MM-DD HH:MM:SS"
        ),
    )
}

impl Timestamp {
    pub const UNIX_EPOCH: Self = Self(0);

    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// 当前时间（来自注入的时钟）。
    pub fn now(clock: &dyn Clock) -> Self {
        clock.now()
    }

    pub const fn plus_millis(self, ms: i64) -> Self {
        Self(self.0 + ms)
    }

    pub const fn minus_millis(self, ms: i64) -> Self {
        Self(self.0 - ms)
    }

    /// 两个时间点之间的间隔毫秒数（可能为负）。
    pub const fn saturating_diff_millis(self, other: Self) -> i64 {
        self.0.saturating_sub(other.0)
    }

    pub fn parse_rfc3339(s: &str) -> Result<Self, AppError> {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| Self(dt.timestamp_millis()))
            .map_err(|e| {
                AppError::new(
                    ErrorCode::DomainInvalidTimestamp,
                    format!("invalid RFC 3339 timestamp {s:?}: {e}"),
                )
            })
    }

    /// 宽松解析前端传来的时间边界：毫秒数（`dayjs().valueOf()`）、
    /// RFC 3339 / ISO 字符串（`toISOString()`）、兼容层的
    /// `YYYY-MM-DD HH:MM:SS`（既有历史兼容格式）—— 三种都在真实调用里出现。
    ///
    /// 只认一种的后果不是报错而是**静默返回空**（首页任务列表与热力图直接变空），
    /// 因此宁可宽容，但绝不猜：解析不出来返回 [`ErrorCode::DomainInvalidTimestamp`]。
    pub fn parse_flexible(raw: &str) -> Result<Self, AppError> {
        let text = raw.trim();
        if text.is_empty() {
            return Err(invalid_timestamp(raw));
        }

        // 1) 毫秒数（纯十进制整数，允许负号）
        if let Ok(millis) = text.parse::<i64>() {
            return Ok(Self(millis));
        }

        // 2) RFC 3339 / ISO 8601
        if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
            return Ok(Self(parsed.timestamp_millis()));
        }

        // 3) 兼容层格式：`YYYY-MM-DD HH:MM:SS`（按 UTC 解释）
        let normalised = text.replace(' ', "T");
        let with_zone = if normalised.ends_with('Z') || normalised.contains('+') {
            normalised
        } else {
            format!("{normalised}Z")
        };
        if let Ok(parsed) = DateTime::parse_from_rfc3339(&with_zone) {
            return Ok(Self(parsed.timestamp_millis()));
        }

        Err(invalid_timestamp(raw))
    }

    pub fn to_rfc3339(self) -> String {
        self.utc()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    /// 该瞬间在指定 IANA 时区下的本地日期。
    pub fn to_local_date(self, tz: &str) -> Result<NaiveDate, AppError> {
        let tz = parse_tz(tz)?;
        Ok(self.utc().with_timezone(&tz).date_naive())
    }

    /// 兼容层的 `DATETIME` 文本（UTC 的 `YYYY-MM-DD HH:MM:SS`）。
    ///
    /// 这类列用的是既有兼容格式，读取方（`dayjs`）也按它解析。
    /// **不要写整数毫秒**：`DATETIME` 是 NUMERIC 亲和，写进去就是 INTEGER，
    /// 按整数写入会变成 NUMERIC 亲和，读出来按字符串解析直接报类型错误。
    pub fn to_legacy_datetime(self) -> String {
        self.format_in_tz("UTC", "%Y-%m-%d %H:%M:%S")
            .unwrap_or_else(|_| self.to_rfc3339())
    }

    /// 该瞬间在指定 IANA 时区下的本地时间字符串。
    ///
    /// 用于给人看的展示（总结正文、时间线标签）。
    /// **不要**用它写回数据库：存储一律 UTC 毫秒，本地时间只是展示格式。
    pub fn format_in_tz(self, tz: &str, format: &str) -> Result<String, AppError> {
        let tz = parse_tz(tz)?;
        Ok(self.utc().with_timezone(&tz).format(format).to_string())
    }

    /// 指定**本地日期**的 `[start, end)` 边界（UTC 时间戳）。
    ///
    /// 与 `day_bounds` 的区别：后者要一个时刻、算它落在哪一天；
    /// 这里直接给日期 —— 生成「昨天的日报」时需要它，
    /// 而「昨天」不能用 `now - 24h` 算（DST 日不是 24 小时）。
    pub fn day_bounds_for(date: NaiveDate, tz: &str) -> Result<(Self, Self), AppError> {
        let tz = parse_tz(tz)?;
        let start = local_midnight(tz, date)?;
        let next_date = date.succ_opt().ok_or_else(|| {
            AppError::new(
                ErrorCode::DomainInvariantViolated,
                format!("no successor date for {date}"),
            )
        })?;
        let end = local_midnight(tz, next_date)?;
        Ok((start, end))
    }

    /// 该瞬间所在「本地日」的 `[start, end)` 边界（UTC 时间戳）。
    ///
    /// DST 安全：
    /// - 春令时本地午夜不存在 → 取第一个存在的时刻（该日不足 24h）
    /// - 冬令时本地午夜重复 → 取**较早**的一个（该日超过 24h）
    ///
    /// 这是日边界与跨天阶段切分的唯一入口。
    pub fn day_bounds(self, tz: &str) -> Result<(Self, Self), AppError> {
        let tz = parse_tz(tz)?;
        let date = self.utc().with_timezone(&tz).date_naive();

        let start = local_midnight(tz, date)?;
        let next_date = date.succ_opt().ok_or_else(|| {
            AppError::new(
                ErrorCode::DomainInvariantViolated,
                format!("no successor date for {date}"),
            )
        })?;
        let end = local_midnight(tz, next_date)?;

        Ok((start, end))
    }

    fn utc(self) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp_millis(self.0).unwrap_or_else(|| {
            // 超出 chrono 可表示范围时退回 epoch，而不是 panic：
            // 一条脏数据不应该让整个后台任务崩掉。
            DateTime::<Utc>::from_timestamp_millis(0).expect("epoch is always representable")
        })
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

/// 默认值是 Unix epoch（而非「当前时间」）。
/// 这样 `TestClock::default()` 是一个**确定的**起点，测试不会因为跑得快慢而不同。
impl Default for Timestamp {
    fn default() -> Self {
        Self::UNIX_EPOCH
    }
}

fn parse_tz(name: &str) -> Result<Tz, AppError> {
    name.parse::<Tz>().map_err(|_| {
        AppError::new(
            ErrorCode::ConfigUnknownTimezone,
            format!("unknown IANA timezone {name:?}"),
        )
    })
}

/// 某个本地日期的「本地午夜」对应的 UTC 瞬间。
fn local_midnight(tz: Tz, date: NaiveDate) -> Result<Timestamp, AppError> {
    let mut naive = date
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is always a valid time");

    // 最多向前扫描 3 小时（按分钟）。真实的 DST 跳跃不会超过 2 小时。
    for _ in 0..=(3 * 60) {
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(dt) => return Ok(Timestamp(dt.timestamp_millis())),
            LocalResult::Ambiguous(earliest, _) => {
                return Ok(Timestamp(earliest.timestamp_millis()))
            }
            LocalResult::None => naive += chrono::Duration::minutes(1),
        }
    }

    Err(AppError::new(
        ErrorCode::DomainInvariantViolated,
        format!("could not resolve local midnight for {date} in {tz}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_and_minus_millis_are_inverses() {
        let t = Timestamp::from_millis(1_000);
        assert_eq!(t.plus_millis(500).minus_millis(500), t);
    }

    #[test]
    fn utc_is_always_accepted_as_a_timezone_name() {
        let t = Timestamp::from_millis(0);
        assert_eq!(t.to_local_date("UTC").unwrap().to_string(), "1970-01-01");
    }
}
