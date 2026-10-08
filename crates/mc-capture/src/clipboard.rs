//! 剪贴板文本采集。
//!
//! 与截图源不同，剪贴板给的是**文本**：存储层已经支持这种形态
//! （`NewObservation.text_content` / `text_origin`，`kind = "clipboard"`），所以这里
//! 只负责「读出来 + 规整 + 去重」，不掺进面向图像帧的 `CaptureSource` 语义。
//!
//! 隐私：剪贴板是最容易夹带密钥与个人信息的来源，因此① 默认**不采**（必须在
//! `capture.sources` 里显式写上 `clipboard`）；② 只保留文本、长度截断；③ 落库前
//! 仍走既有的黑名单与脱敏判定。

/// 单条剪贴板文本的字节上限。超出部分截断并留下标记，避免一次复制大文件正文
/// 就把库撑大。
pub const MAX_BYTES: usize = 8 * 1024;

/// 读取剪贴板文本。读不到（非文本、空、命令不可用）返回 `None`。
pub fn read_text() -> Option<String> {
    let output = std::process::Command::new("pbpaste").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    normalize(&text, MAX_BYTES)
}

/// 规整：去掉首尾空白；全空白视为没内容；按**字符边界**截断。
pub fn normalize(text: &str, max_bytes: usize) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= max_bytes {
        return Some(trimmed.to_string());
    }

    // 不能按字节切：多字节字符被切断会得到无效 UTF-8（写进库就是坏数据）
    let mut end = max_bytes;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}…（已截断）", &trimmed[..end]))
}

/// 是否需要入库：空内容不入库；与上一次内容相同也不入库（复制粘贴常连续触发，
/// 不去重会把同一段文本写成几十条观测）。
pub fn is_new(previous: Option<&str>, current: &str) -> bool {
    previous != Some(current)
}

/// 内容指纹（FNV-1a 64 位）：用于观测的 `phash` 与幂等键。
///
/// 截图源用感知哈希去重；文本用内容哈希即可 —— 同样的文本本来就该是同一条观测。
pub fn content_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{content_hash, is_new, normalize};

    #[test]
    fn blank_clipboard_is_not_stored() {
        assert!(normalize("", 1024).is_none());
        assert!(normalize("   \n\t ", 1024).is_none());
    }

    #[test]
    fn keeps_short_text_verbatim_after_trim() {
        assert_eq!(normalize("  hello  ", 1024).as_deref(), Some("hello"));
    }

    #[test]
    fn truncates_on_a_character_boundary() {
        // 全是三字节汉字：上限 10 字节时不能切出半个字
        let text = "汉字汉字汉字";
        let result = normalize(text, 10).expect("有内容");
        assert!(result.ends_with("…（已截断）"), "{result}");
        assert!(result.starts_with("汉字"), "{result}");
        // 截断后的前缀必须是完整字符
        let prefix = result.trim_end_matches("…（已截断）");
        assert_eq!(prefix.chars().count(), 3, "{result}");
    }

    #[test]
    fn content_hash_is_stable_and_distinguishes_texts() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
    }

    #[test]
    fn deduplicates_against_the_previous_value() {
        assert!(is_new(None, "a"));
        assert!(is_new(Some("a"), "b"));
        assert!(!is_new(Some("a"), "a"));
    }
}
