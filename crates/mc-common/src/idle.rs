//! 用户空闲时长。
//!
//! 空闲是「要不要降频」的输入：用户离开后不必按原频率截图（省电、省 token）。
//! 判定走 `ioreg -c IOHIDSystem` 的 `HIDIdleTime`（纳秒），与 `disk.rs` / `session.rs`
//! 同一思路：不引平台依赖，读系统既有输出，解析是纯函数因而可测。
//!
//! 读不出来返回 `None`，调用方按「不空闲」处理 —— 探测失败不该让采集降频。

/// 解析 `ioreg` 输出里的 `HIDIdleTime`（纳秒）。
pub fn parse_idle_nanos(text: &str) -> Option<u64> {
    for line in text.lines() {
        if !line.contains("\"HIDIdleTime\"") {
            continue;
        }
        let value = line.split('=').nth(1)?.trim();
        return value.parse().ok();
    }
    None
}

/// 当前用户空闲秒数。探测失败返回 `None`（调用方按不空闲处理）。
pub fn idle_secs() -> Option<u64> {
    let output = std::process::Command::new("ioreg")
        .args(["-c", "IOHIDSystem"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_idle_nanos(&String::from_utf8_lossy(&output.stdout)).map(|nanos| nanos / 1_000_000_000)
}

#[cfg(test)]
mod tests {
    use super::parse_idle_nanos;

    #[test]
    fn parses_the_real_output_shape() {
        // 本机 ioreg 实际输出（约 399 秒）
        let text = "    | | |   \"HIDIdleTime\" = 399229025000\n";
        assert_eq!(parse_idle_nanos(text), Some(399_229_025_000));
    }

    #[test]
    fn zero_when_the_user_just_typed() {
        let text = "    | | |   \"HIDIdleTime\" = 0\n";
        assert_eq!(parse_idle_nanos(text), Some(0));
    }

    #[test]
    fn unknown_when_the_key_is_absent_or_unparsable() {
        assert_eq!(parse_idle_nanos("  \"SomethingElse\" = 12\n"), None);
        assert_eq!(parse_idle_nanos("  \"HIDIdleTime\" = not-a-number\n"), None);
    }
}
