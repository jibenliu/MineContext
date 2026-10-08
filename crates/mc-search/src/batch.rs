//! 批量切分：把 N 条输入切成若干不超过 `limit` 的连续区间。
//!
//! 纯函数，故意不碰任何 I/O —— 这样「批次计划」可以被穷举测试，
//! 而上传顺序、断点续传、失败重试都只需建立在「计划是确定的」之上。
//!
//! 上限来自配置（各家 embedding 端点不同，常见 64 / 100 / 2048），
//! **不是常量**：把厂商限制写死在代码里，换模型就会整批失败。

use std::ops::Range;

/// 把 `total` 条输入切成 `limit` 条一批的连续区间。
///
/// - `total == 0` → 空计划（不产生空请求）
/// - `limit == 0` → `Err`（调用方配置写错了，不能静默变成死循环或空计划）
/// - 整除时不产生空尾巴
pub fn plan_batches(total: usize, limit: usize) -> Result<Vec<Range<usize>>, String> {
    if limit == 0 {
        return Err("embedding 批量上限不能为 0，请检查配置".to_string());
    }

    let mut batches = Vec::with_capacity(total.div_ceil(limit));
    let mut start = 0;
    while start < total {
        let end = (start + limit).min(total);
        batches.push(start..end);
        start = end;
    }
    Ok(batches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_hint_matches_number_of_batches() {
        let batches = plan_batches(1000, 64).unwrap();
        assert_eq!(batches.len(), 1000usize.div_ceil(64));
    }
}
