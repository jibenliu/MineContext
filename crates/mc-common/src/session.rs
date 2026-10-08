//! 会话状态：屏幕是否处于锁定状态。
//!
//! 锁屏画面属于隐私内容，继续采集等于把用户离开后的桌面录下来；锁屏期间也几乎没有
//! 可用活动。判定走 `ioreg`（与 `disk.rs` 用 `df` 同一思路：不引入平台依赖，读系统
//! 既有输出，解析函数是纯函数因而可测）。键名随 macOS 版本不同：本机实测是
//! `"IOConsoleLocked"`，资料里常见的是 `"CGSSessionScreenIsLocked"` —— 两个都认，
//! 否则探测会永远返回未知、功能等于没做。
//!
//! 读不出来一律返回 `None`，调用方按「未锁定」处理：宁可多采，也不能因为探测
//! 失败把采集整体停掉。

/// 解析 `ioreg` 输出里的锁屏标志。
///
/// 命中 `= Yes` 返回 `Some(true)`，`= No` 返回 `Some(false)`，没有这两个键返回 `None`。
pub fn parse_screen_locked(text: &str) -> Option<bool> {
    for line in text.lines() {
        let matched =
            line.contains("\"CGSSessionScreenIsLocked\"") || line.contains("\"IOConsoleLocked\"");
        if !matched {
            continue;
        }
        let value = line.split('=').nth(1)?.trim();
        if value.starts_with("Yes") {
            return Some(true);
        }
        if value.starts_with("No") {
            return Some(false);
        }
    }
    None
}

/// 当前屏幕是否锁定。探测失败返回 `None`（调用方按未锁定处理）。
pub fn screen_locked() -> Option<bool> {
    let output = std::process::Command::new("ioreg")
        .args(["-n", "Root", "-d1"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_screen_locked(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::parse_screen_locked;

    #[test]
    fn detects_a_locked_screen() {
        let text = "  | |   \"CGSSessionScreenIsLocked\" = Yes\n  | |   \"kCGSSessionOnConsoleKey\" = Yes\n";
        assert_eq!(parse_screen_locked(text), Some(true));
    }

    #[test]
    fn detects_the_console_locked_key_used_by_current_macos() {
        // 本机 ioreg 实际输出（未锁屏）
        let text = "      \"IOConsoleLocked\" = No\n";
        assert_eq!(parse_screen_locked(text), Some(false));

        let locked = "      \"IOConsoleLocked\" = Yes\n";
        assert_eq!(parse_screen_locked(locked), Some(true));
    }

    #[test]
    fn unknown_when_the_key_is_absent() {
        // 没有这两个键（例如系统版本不同）时返回 None，由调用方按未锁定处理
        let text = "  | |   \"kCGSSessionOnConsoleKey\" = Yes\n  | |   \"IOConsoleUsers\" = ()\n";
        assert_eq!(parse_screen_locked(text), None);
    }

    #[test]
    fn ignores_similar_keys() {
        // 只有引号包住的完整键名才算；相似名字不该被误读
        let text = "  | |   \"CGSSessionScreenIsLockedOther\" = Yes\n";
        assert_eq!(parse_screen_locked(text), None);
    }
}
