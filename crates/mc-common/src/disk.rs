//! 磁盘余量：采集侧的「快满了就降级」判据。
//!
//! 为什么不用 `statvfs`：手写 `extern "C"` 的结构体布局要与平台 ABI 完全一致，
//! 一旦错位就是读垃圾值 —— 而这里的用途只是**粗粒度阈值**（几百 MB 级别），
//! 因此走 `df -k -P` 解析。调用频率是分钟级（不是每 tick），开销可以忽略。
//!
//! 两条硬约束：
//! 1. **读不到余量时按「有空间」处理**：磁盘查询失败不该让采集停摆，
//!    真正的写失败仍会照常记录（错误码 `storage_disk_full`）；
//! 2. 阈值**严格小于**才拦：边界写错会让用户在临界点反复启停采集。

/// 采集降级阈值：低于这个余量就暂停采集（512 MiB）。
///
/// 为什么是 512 MiB：一次保留策略轮转、一次 SQLite checkpoint 与几天的截图
/// 都需要余量；比这更小的时候，先停采集比「写到一半失败」对用户更好。
pub const MIN_FREE_BYTES: u64 = 512 * 1024 * 1024;

/// 是否应当因为磁盘余量暂停采集。
pub fn should_pause(free_bytes: u64, min_free_bytes: u64) -> bool {
    free_bytes < min_free_bytes
}

/// 解析 `df -k -P <path>` 的输出（POSIX 格式：一行表头 + 一行数据，单位 KB）。
///
/// `-P` 保证是「每个文件系统一行」，因此不会因为长设备名换行而错位。
pub fn parse_df_k(output: &str) -> Option<u64> {
    let line = output
        .lines()
        .skip(1)
        .find(|line| !line.trim().is_empty())?;
    let available_kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(available_kb.saturating_mul(1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_posix_df_output() {
        let output = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      /dev/disk3s5 971350180 600000000 351234567 64% /System/Volumes/Data\n";
        assert_eq!(parse_df_k(output), Some(351_234_567 * 1024));
    }

    #[test]
    fn long_device_names_do_not_break_the_parse() {
        // `-P` 下再长的设备名也只占一行
        let output = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      /dev/disk3s5s1s1s1s1s1s1s1 971350180 971349000 1000 100% /Volumes/Very Long Name\n";
        assert_eq!(parse_df_k(output), Some(1000 * 1024));
    }

    #[test]
    fn malformed_output_is_none_rather_than_a_wrong_number() {
        assert_eq!(parse_df_k(""), None);
        assert_eq!(parse_df_k("Filesystem 1024-blocks Used Available\n"), None);
        // 列数不足时不能猜：按位置取第 4 列是 POSIX `-P` 的语义，
        // 少于 4 列说明输出不是我们认识的样子
        assert_eq!(
            parse_df_k(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 100 50\n"
            ),
            None
        );
    }

    #[test]
    fn threshold_is_strictly_below() {
        assert!(should_pause(MIN_FREE_BYTES - 1, MIN_FREE_BYTES));
        assert!(!should_pause(MIN_FREE_BYTES, MIN_FREE_BYTES));
        assert!(!should_pause(MIN_FREE_BYTES + 1, MIN_FREE_BYTES));
    }
}

/// 读某个路径所在卷的可用字节数（跑一次 `df -k -P`）。
///
/// 读不到返回 `None`：调用方按「有空间」处理，绝不因为查不到余量就停摆。
pub fn free_bytes(path: impl AsRef<std::path::Path>) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-k")
        .arg("-P")
        .arg(path.as_ref())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_df_k(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod free_space_tests {
    use super::*;

    #[test]
    fn reads_free_space_of_an_existing_volume() {
        let dir = tempfile::tempdir().unwrap();
        let free = free_bytes(dir.path()).expect("df 应当能读到临时目录所在卷");
        assert!(free > 0, "可用空间应当大于 0，实际 {free}");
    }

    #[test]
    fn missing_path_yields_none_instead_of_panicking() {
        assert!(free_bytes("/definitely/not/here").is_none());
    }
}
